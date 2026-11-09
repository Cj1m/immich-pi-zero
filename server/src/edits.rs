//! Crop, rotate, and mirror. One row per action. The edited JPEG is a second preview.

use chrono::{Duration, SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};
use crate::media::sync_asset_json;
use crate::thumbs::{self, StillEdit};

const AUDIT_DAYS: i64 = 31;

#[derive(Clone)]
pub(crate) struct EditAction {
    pub action: String,
    pub parameters: Value,
}

struct EditRecord {
    id: String,
    action: String,
    parameters: String,
    sequence: i64,
}

impl App {
    pub async fn replace_asset_edits(
        &self,
        caller: &str,
        asset_id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        let edits = parse_edits(body)?;
        self.write_edits(caller, asset_id, edits, "asset.edit.create")
            .await
    }

    pub async fn clear_asset_edits(&self, caller: &str, asset_id: &str) -> Result<(), DbError> {
        self.write_edits(caller, asset_id, Vec::new(), "asset.edit.delete")
            .await?;
        Ok(())
    }

    pub async fn read_asset_edits(&self, caller: &str, asset_id: &str) -> Result<Value, DbError> {
        self.require_edit_access(caller, asset_id, EditAccess::Read)
            .await?;
        self.edit_response(asset_id).await
    }

    pub(crate) async fn edit_actions(&self, asset_id: &str) -> Result<Vec<EditAction>, DbError> {
        let rows = self.edit_rows(asset_id).await?;
        Ok(rows
            .into_iter()
            .map(|row| EditAction {
                action: row.action,
                parameters: parse_parameters(&row.parameters),
            })
            .collect())
    }

    pub(crate) async fn edit_documents(&self, asset_id: &str) -> Result<Value, DbError> {
        let response = self.edit_response(asset_id).await?;
        Ok(response["edits"].clone())
    }

    /// Drop edit-delete rows older than 31 days.
    pub async fn prune_edit_audits(&self) -> Result<(), DbError> {
        let cutoff = (Utc::now() - Duration::days(AUDIT_DAYS))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        sqlx::query("DELETE FROM asset_edit_audit WHERE deletedAt < ?")
            .bind(cutoff)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn write_edits(
        &self,
        caller: &str,
        asset_id: &str,
        edits: Vec<EditAction>,
        permission: &'static str,
    ) -> Result<Value, DbError> {
        let access = if permission.ends_with("delete") {
            EditAccess::Delete
        } else {
            EditAccess::Create
        };
        let record = self.require_edit_access(caller, asset_id, access).await?;
        if !edits.is_empty() {
            reject_uneditable(&record, &edits)?;
        }
        let previous = self.edit_rows(asset_id).await?;
        let now = now_iso();
        let mut stored = Vec::new();
        let mut transaction = self.pool().begin().await?;
        for row in &previous {
            sqlx::query(
                "INSERT INTO asset_edit_audit (id, editId, assetId, deletedAt) VALUES (?, ?, ?, ?)",
            )
            .bind(new_update_id())
            .bind(&row.id)
            .bind(asset_id)
            .bind(&now)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("DELETE FROM asset_edit WHERE assetId = ?")
            .bind(asset_id)
            .execute(&mut *transaction)
            .await?;
        for (sequence, edit) in edits.iter().enumerate() {
            let id = new_id();
            let update_id = new_update_id();
            let parameters = edit.parameters.to_string();
            sqlx::query(
                "INSERT INTO asset_edit (
                    id, assetId, action, parameters, sequence, updatedAt, updateId
                 ) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(asset_id)
            .bind(&edit.action)
            .bind(&parameters)
            .bind(sequence as i64)
            .bind(&now)
            .bind(&update_id)
            .execute(&mut *transaction)
            .await?;
            stored.push(EditRecord {
                id,
                action: edit.action.clone(),
                parameters,
                sequence: sequence as i64,
            });
        }
        sqlx::query(
            "UPDATE asset SET isEdited = ?, updatedAt = ?, updateId = ? WHERE id = ?",
        )
        .bind(i64::from(!stored.is_empty()))
        .bind(&now)
        .bind(new_update_id())
        .bind(asset_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        if let Err(error) = self.paint_edits(asset_id, &edits).await {
            eprintln!("edit thumbnail: {error:?}");
        }
        self.finish_edit(asset_id, &stored).await?;
        self.edit_response(asset_id).await
    }

    async fn paint_edits(&self, asset_id: &str, edits: &[EditAction]) -> Result<(), DbError> {
        let Some(source) = self.edit_source(asset_id).await? else {
            return Ok(());
        };
        let data_dir = self.data_dir().to_path_buf();
        let asset = asset_id.to_string();
        let steps = still_edits(edits);
        let orientation = source.orientation;
        let original = source.original;
        if steps.is_empty() {
            thumbs::remove_edited_stills(&data_dir, &asset);
            sqlx::query(
                "DELETE FROM asset_file WHERE assetId = ? AND type IN ('thumbnail-edited', 'preview-edited')",
            )
            .bind(asset_id)
            .execute(self.pool())
            .await?;
            let size = tokio::task::spawn_blocking(move || {
                thumbs::oriented_size(&original, orientation)
            })
            .await
            .unwrap_or(None);
            if let Some((width, height)) = size {
                self.store_edited_size(asset_id, width, height).await?;
            }
            self.reveal_regions(asset_id).await?;
            return Ok(());
        }
        let written = tokio::task::spawn_blocking(move || {
            thumbs::write_edited_stills(&data_dir, &asset, &original, orientation, &steps)
        })
        .await
        .unwrap_or(None);
        let Some((width, height)) = written else {
            return Err(DbError::BadRequest("Asset dimensions are not available for editing"));
        };
        for (kind, file_name) in [
            ("thumbnail-edited", "thumbnail_edited.jpg"),
            ("preview-edited", "preview_edited.jpg"),
        ] {
            let relative = format!("thumbs/{asset_id}/{file_name}");
            sqlx::query(
                "INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, ?, ?)
                 ON CONFLICT(assetId, type) DO UPDATE SET path = excluded.path",
            )
            .bind(new_id())
            .bind(asset_id)
            .bind(kind)
            .bind(relative)
            .execute(self.pool())
            .await?;
        }
        self.store_edited_size(asset_id, width, height).await?;
        self.hide_cropped_regions(asset_id, edits, source.bounds).await?;
        Ok(())
    }

    async fn store_edited_size(&self, asset_id: &str, width: i64, height: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE asset SET width = ?, height = ?, updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(width)
            .bind(height)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(asset_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn finish_edit(&self, asset_id: &str, edits: &[EditRecord]) -> Result<(), DbError> {
        let Some(asset) = self.sync_asset_value(asset_id).await? else {
            return Ok(());
        };
        let edit = Value::Array(
            edits
                .iter()
                .map(|row| {
                    json!({
                        "id": row.id,
                        "assetId": asset_id,
                        "action": row.action,
                        "parameters": parse_parameters(&row.parameters),
                        "sequence": row.sequence,
                    })
                })
                .collect(),
        );
        let packet = format!("42{}", json!(["AssetEditReadyV2", { "asset": asset, "edit": edit }]));
        let owner = asset["ownerId"].as_str().unwrap_or("");
        self.send_user_event(owner, &packet);
        Ok(())
    }

    async fn edit_source(&self, asset_id: &str) -> Result<Option<EditSource>, DbError> {
        let row: Option<EditSourceRow> = sqlx::query_as(
            "SELECT asset.originalPath AS path, asset_exif.orientation AS orientation,
                    asset_exif.exifImageWidth AS width, asset_exif.exifImageHeight AS height
             FROM asset
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.id = ?",
        )
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let EditSourceRow {
            path,
            orientation,
            width,
            height,
        } = row;
        let image_orientation = orientation
            .as_deref()
            .and_then(|value| value.parse::<u16>().ok());
        let bounds = match (width, height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => Some(oriented_bounds(
                width,
                height,
                parse_orientation(orientation.as_deref()),
            )),
            _ => None,
        };
        Ok(Some(EditSource {
            original: self.data_dir().join("objects").join(path),
            orientation: image_orientation,
            bounds,
        }))
    }

    async fn reveal_regions(&self, asset_id: &str) -> Result<(), DbError> {
        let now = now_iso();
        sqlx::query(
            "UPDATE asset_face SET isVisible = 1, updatedAt = ?, updateId = ?
             WHERE assetId = ? AND deletedAt IS NULL AND isVisible = 0",
        )
        .bind(&now)
        .bind(new_update_id())
        .bind(asset_id)
        .execute(self.pool())
        .await?;
        sqlx::query("UPDATE asset_ocr SET isVisible = 1, updatedAt = ?, updateId = ? WHERE assetId = ? AND isVisible = 0")
            .bind(&now)
            .bind(new_update_id())
            .bind(asset_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn hide_cropped_regions(
        &self,
        asset_id: &str,
        edits: &[EditAction],
        bounds: Option<(i64, i64)>,
    ) -> Result<(), DbError> {
        let Some(crop) = edits.iter().find(|edit| edit.action == "crop") else {
            return self.reveal_regions(asset_id).await;
        };
        let Some((width, height)) = bounds else {
            return Ok(());
        };
        let crop_box = (
            crop.parameters["x"].as_f64().unwrap_or(0.0),
            crop.parameters["y"].as_f64().unwrap_or(0.0),
            crop.parameters["x"].as_f64().unwrap_or(0.0) + crop.parameters["width"].as_f64().unwrap_or(0.0),
            crop.parameters["y"].as_f64().unwrap_or(0.0) + crop.parameters["height"].as_f64().unwrap_or(0.0),
        );
        let faces: Vec<StoredFace> = sqlx::query_as(
            "SELECT id, imageWidth AS image_width, imageHeight AS image_height,
                    boundingBoxX1 AS x1, boundingBoxY1 AS y1, boundingBoxX2 AS x2,
                    boundingBoxY2 AS y2, isVisible AS visible
             FROM asset_face WHERE assetId = ? AND deletedAt IS NULL",
        )
        .bind(asset_id)
        .fetch_all(self.pool())
        .await?;
        let now = now_iso();
        for face in faces {
            let scaled = scale_box(
                (face.x1, face.y1, face.x2, face.y2),
                (face.image_width, face.image_height),
                (width, height),
            );
            let next = i64::from(overlap(scaled, crop_box) >= 0.5);
            if next == face.visible {
                continue;
            }
            sqlx::query(
                "UPDATE asset_face SET isVisible = ?, updatedAt = ?, updateId = ? WHERE id = ?",
            )
            .bind(next)
            .bind(&now)
            .bind(new_update_id())
            .bind(face.id)
            .execute(self.pool())
            .await?;
        }
        let regions: Vec<StoredOcr> = sqlx::query_as(
            "SELECT id, x1, y1, x2, y2, x3, y3, x4, y4, isVisible AS visible
             FROM asset_ocr WHERE assetId = ?",
        )
        .bind(asset_id)
        .fetch_all(self.pool())
        .await?;
        for region in regions {
            let left = region.x1.min(region.x2).min(region.x3).min(region.x4);
            let top = region.y1.min(region.y2).min(region.y3).min(region.y4);
            let right = region.x1.max(region.x2).max(region.x3).max(region.x4);
            let bottom = region.y1.max(region.y2).max(region.y3).max(region.y4);
            let next = i64::from(overlap((left, top, right, bottom), crop_box) >= 0.5);
            if next == region.visible {
                continue;
            }
            sqlx::query("UPDATE asset_ocr SET isVisible = ?, updatedAt = ?, updateId = ? WHERE id = ?")
                .bind(next)
                .bind(&now)
                .bind(new_update_id())
                .bind(region.id)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    async fn edit_rows(&self, asset_id: &str) -> Result<Vec<EditRecord>, DbError> {
        let rows = sqlx::query_as::<_, EditRowSql>(
            "SELECT id, action, parameters, sequence
             FROM asset_edit WHERE assetId = ? ORDER BY sequence",
        )
        .bind(asset_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| EditRecord {
                id: row.id,
                action: row.action,
                parameters: row.parameters,
                sequence: row.sequence,
            })
            .collect())
    }

    async fn edit_response(&self, asset_id: &str) -> Result<Value, DbError> {
        let rows = self.edit_rows(asset_id).await?;
        let edits: Vec<Value> = rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "action": row.action,
                    "parameters": parse_parameters(&row.parameters),
                })
            })
            .collect();
        Ok(json!({ "assetId": asset_id, "edits": edits }))
    }

    async fn require_edit_access(
        &self,
        caller: &str,
        asset_id: &str,
        access: EditAccess,
    ) -> Result<EditAsset, DbError> {
        if uuid::Uuid::parse_str(asset_id).is_err() {
            return Err(DbError::Validation);
        }
        let row: Option<EditAsset> = sqlx::query_as(
            "SELECT asset.ownerId AS owner_id, asset.type AS asset_type,
                    asset.originalPath AS original_path, asset.originalFileName AS file_name,
                    asset.visibility AS visibility, asset.livePhotoVideoId AS live_photo_video_id,
                    CASE WHEN asset_exif.assetId IS NULL THEN 1 ELSE 0 END AS exif_missing,
                    asset_exif.exifImageWidth AS exif_width,
                    asset_exif.exifImageHeight AS exif_height,
                    asset_exif.orientation AS orientation,
                    asset_exif.projectionType AS projection_type
             FROM asset
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.id = ?",
        )
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else {
            return Err(DbError::BadRequest(access.denied()));
        };
        if row.visibility == "locked" {
            return Err(DbError::BadRequest(access.denied()));
        }
        let allowed = match access {
            EditAccess::Read => {
                row.owner_id == caller || self.album_can_read(caller, asset_id).await?
            }
            EditAccess::Create | EditAccess::Delete => row.owner_id == caller,
        };
        if !allowed {
            return Err(DbError::BadRequest(access.denied()));
        }
        Ok(row)
    }

    async fn album_can_read(&self, caller: &str, asset_id: &str) -> Result<bool, DbError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM album_asset
             JOIN album ON album.id = album_asset.albumId AND album.deletedAt IS NULL
             JOIN album_user ON album_user.albumId = album.id AND album_user.userId = ?
             WHERE album_asset.assetId = ?",
        )
        .bind(caller)
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(found.is_some())
    }
}

struct EditSource {
    original: std::path::PathBuf,
    orientation: Option<u16>,
    bounds: Option<(i64, i64)>,
}

#[derive(sqlx::FromRow)]
struct EditSourceRow {
    path: String,
    orientation: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct StoredFace {
    id: String,
    image_width: i64,
    image_height: i64,
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
    visible: i64,
}

#[derive(sqlx::FromRow)]
struct StoredOcr {
    id: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    x3: f64,
    y3: f64,
    x4: f64,
    y4: f64,
    visible: i64,
}

#[derive(sqlx::FromRow)]
struct EditRowSql {
    id: String,
    action: String,
    parameters: String,
    sequence: i64,
}

#[derive(sqlx::FromRow)]
struct EditAsset {
    owner_id: String,
    asset_type: String,
    original_path: String,
    file_name: String,
    visibility: String,
    live_photo_video_id: Option<String>,
    exif_missing: i64,
    exif_width: Option<i64>,
    exif_height: Option<i64>,
    orientation: Option<String>,
    projection_type: Option<String>,
}

#[derive(Clone, Copy)]
enum EditAccess {
    Read,
    Create,
    Delete,
}

impl EditAccess {
    fn denied(self) -> &'static str {
        match self {
            Self::Read => "Not found or no asset.read access",
            Self::Create => "Not found or no asset.edit.create access",
            Self::Delete => "Not found or no asset.edit.delete access",
        }
    }
}

fn reject_uneditable(asset: &EditAsset, edits: &[EditAction]) -> Result<(), DbError> {
    if asset.exif_missing != 0 {
        return Err(DbError::BadRequest("Asset not found"));
    }
    if ends_with_ignore(&asset.file_name, ".gif") || ends_with_ignore(&asset.original_path, ".gif")
    {
        return Err(DbError::BadRequest("Editing GIF images is not supported"));
    }
    if ends_with_ignore(&asset.file_name, ".svg") || ends_with_ignore(&asset.original_path, ".svg")
    {
        return Err(DbError::BadRequest("Editing SVG images is not supported"));
    }
    if asset.projection_type.as_deref() == Some("EQUIRECTANGULAR")
        || ends_with_ignore(&asset.file_name, ".insp")
        || ends_with_ignore(&asset.original_path, ".insp")
    {
        return Err(DbError::BadRequest("Editing panorama images is not supported"));
    }
    if asset.asset_type != "IMAGE" {
        return Err(DbError::BadRequest("Only images can be edited"));
    }
    if asset.live_photo_video_id.is_some() {
        return Err(DbError::BadRequest("Editing live photos is not supported"));
    }
    let Some(width) = asset.exif_width.filter(|value| *value > 0) else {
        return Err(DbError::BadRequest("Asset dimensions are not available for editing"));
    };
    let Some(height) = asset.exif_height.filter(|value| *value > 0) else {
        return Err(DbError::BadRequest("Asset dimensions are not available for editing"));
    };
    let (width, height) = oriented_bounds(width, height, parse_orientation(asset.orientation.as_deref()));
    if let Some(crop) = edits.iter().find(|edit| edit.action == "crop") {
        if edits[0].action != "crop" {
            return Err(DbError::BadRequest("Crop action must be the first edit action"));
        }
        let x = crop.parameters["x"].as_i64().unwrap_or(0);
        let y = crop.parameters["y"].as_i64().unwrap_or(0);
        let crop_width = crop.parameters["width"].as_i64().unwrap_or(0);
        let crop_height = crop.parameters["height"].as_i64().unwrap_or(0);
        if x + crop_width > width || y + crop_height > height {
            return Err(DbError::BadRequest("Crop parameters are out of bounds"));
        }
    }
    Ok(())
}

pub(crate) fn parse_orientation(value: Option<&str>) -> Option<i32> {
    value.and_then(|text| text.parse().ok())
}

pub(crate) fn oriented_bounds(width: i64, height: i64, orientation: Option<i32>) -> (i64, i64) {
    if matches!(orientation, Some(5 | 6 | 7 | 8 | 90 | -90)) {
        (height, width)
    } else {
        (width, height)
    }
}

fn ends_with_ignore(value: &str, suffix: &str) -> bool {
    value.len() >= suffix.len()
        && value.is_char_boundary(value.len() - suffix.len())
        && value[value.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn parse_edits(body: &Value) -> Result<Vec<EditAction>, DbError> {
    let Some(items) = body.get("edits").and_then(Value::as_array) else {
        return Err(DbError::Validation);
    };
    if items.is_empty() {
        return Err(DbError::Validation);
    }
    let mut edits = Vec::new();
    let mut seen = Vec::new();
    for item in items {
        let action = item
            .get("action")
            .and_then(Value::as_str)
            .ok_or(DbError::Validation)?;
        let parameters = item.get("parameters").ok_or(DbError::Validation)?;
        let parameters = match action {
            "crop" => crop_parameters(parameters)?,
            "rotate" => rotate_parameters(parameters)?,
            "mirror" => mirror_parameters(parameters)?,
            _ => return Err(DbError::Validation),
        };
        let key = if action == "mirror" {
            format!("mirror:{}", parameters["axis"].as_str().unwrap_or(""))
        } else {
            action.to_string()
        };
        if seen.iter().any(|existing: &String| existing == &key) {
            return Err(DbError::BadRequest("Duplicate edit actions are not allowed"));
        }
        seen.push(key);
        edits.push(EditAction {
            action: action.to_string(),
            parameters,
        });
    }
    Ok(edits)
}

fn crop_parameters(value: &Value) -> Result<Value, DbError> {
    let x = json_int(value.get("x")).filter(|number| *number >= 0).ok_or(DbError::Validation)?;
    let y = json_int(value.get("y")).filter(|number| *number >= 0).ok_or(DbError::Validation)?;
    let width = json_int(value.get("width")).filter(|number| *number >= 1).ok_or(DbError::Validation)?;
    let height = json_int(value.get("height")).filter(|number| *number >= 1).ok_or(DbError::Validation)?;
    Ok(json!({ "x": x, "y": y, "width": width, "height": height }))
}

fn rotate_parameters(value: &Value) -> Result<Value, DbError> {
    let Some(angle) = json_int(value.get("angle")) else {
        return Err(DbError::Validation);
    };
    if !matches!(angle, 0 | 90 | 180 | 270) {
        return Err(DbError::BadRequest(
            "Angle must be one of the following values: 0, 90, 180, 270",
        ));
    }
    Ok(json!({ "angle": angle }))
}

fn mirror_parameters(value: &Value) -> Result<Value, DbError> {
    let axis = value.get("axis").and_then(Value::as_str).ok_or(DbError::Validation)?;
    if axis != "horizontal" && axis != "vertical" {
        return Err(DbError::Validation);
    }
    Ok(json!({ "axis": axis }))
}

fn json_int(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

fn parse_parameters(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or(Value::Null)
}

fn still_edits(edits: &[EditAction]) -> Vec<StillEdit> {
    edits
        .iter()
        .filter_map(|edit| match edit.action.as_str() {
            "crop" => Some(StillEdit::Crop {
                x: edit.parameters["x"].as_u64()? as u32,
                y: edit.parameters["y"].as_u64()? as u32,
                width: edit.parameters["width"].as_u64()? as u32,
                height: edit.parameters["height"].as_u64()? as u32,
            }),
            "rotate" => Some(StillEdit::Rotate(edit.parameters["angle"].as_u64()? as u32)),
            "mirror" if edit.parameters["axis"] == "horizontal" => Some(StillEdit::MirrorHorizontal),
            "mirror" => Some(StillEdit::MirrorVertical),
            _ => None,
        })
        .collect()
}

fn scale_box(
    edges: (i64, i64, i64, i64),
    image: (i64, i64),
    target: (i64, i64),
) -> (f64, f64, f64, f64) {
    let width = image.0.max(1) as f64;
    let height = image.1.max(1) as f64;
    (
        edges.0 as f64 / width * target.0 as f64,
        edges.1 as f64 / height * target.1 as f64,
        edges.2 as f64 / width * target.0 as f64,
        edges.3 as f64 / height * target.1 as f64,
    )
}

fn overlap(face: (f64, f64, f64, f64), crop: (f64, f64, f64, f64)) -> f64 {
    let x1 = face.0.max(crop.0);
    let y1 = face.1.max(crop.1);
    let x2 = face.2.min(crop.2);
    let y2 = face.3.min(crop.3);
    let area = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let face_area = (face.2 - face.0) * (face.3 - face.1);
    if face_area <= 0.0 {
        0.0
    } else {
        area / face_area
    }
}

pub(crate) struct PreviewBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub image_width: f64,
}

pub(crate) fn invert_face_box(
    face: PreviewBox,
    edits: &[EditAction],
    edited_width: f64,
    edited_height: f64,
) -> (f64, f64, f64, f64) {
    if edits.is_empty() || face.image_width == 0.0 {
        return (face.x, face.y, face.x + face.width, face.y + face.height);
    }
    let scale = edited_width / face.image_width;
    let mut points = [
        (face.x * scale, face.y * scale),
        ((face.x + face.width) * scale, (face.y + face.height) * scale),
    ];
    let mut current_width = edited_width;
    let mut current_height = edited_height;
    for edit in edits.iter().rev() {
        if edit.action == "rotate" {
            let angle = edit.parameters["angle"].as_f64().unwrap_or(0.0);
            let swapped = angle == 90.0 || angle == 270.0;
            let new_width = if swapped { current_height } else { current_width };
            let new_height = if swapped { current_width } else { current_height };
            let matrix = compose(
                translate(new_width / 2.0, new_height / 2.0),
                rotate(-angle.to_radians()),
                translate(-current_width / 2.0, -current_height / 2.0),
            );
            points = points.map(|(px, py)| apply(matrix, px, py));
            current_width = new_width;
            current_height = new_height;
        } else if edit.action == "mirror" {
            let flip = if edit.parameters["axis"] == "horizontal" {
                flip_y()
            } else {
                flip_x()
            };
            let matrix = compose(
                translate(current_width / 2.0, current_height / 2.0),
                flip,
                translate(-current_width / 2.0, -current_height / 2.0),
            );
            points = points.map(|(px, py)| apply(matrix, px, py));
        }
    }
    if let Some(crop) = edits.iter().find(|edit| edit.action == "crop") {
        let crop_x = crop.parameters["x"].as_f64().unwrap_or(0.0);
        let crop_y = crop.parameters["y"].as_f64().unwrap_or(0.0);
        points = points.map(|(px, py)| (px + crop_x, py + crop_y));
    }
    let left = points[0].0.min(points[1].0);
    let top = points[0].1.min(points[1].1);
    let right = points[0].0.max(points[1].0);
    let bottom = points[0].1.max(points[1].1);
    (left, top, right, bottom)
}

#[derive(Clone, Copy)]
struct Mat {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

fn translate(x: f64, y: f64) -> Mat {
    Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: x, f: y }
}

fn rotate(radians: f64) -> Mat {
    let (sin, cos) = radians.sin_cos();
    Mat { a: cos, b: sin, c: -sin, d: cos, e: 0.0, f: 0.0 }
}

fn flip_x() -> Mat {
    Mat { a: -1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
}

fn flip_y() -> Mat {
    Mat { a: 1.0, b: 0.0, c: 0.0, d: -1.0, e: 0.0, f: 0.0 }
}

fn compose(first: Mat, second: Mat, third: Mat) -> Mat {
    mul(first, mul(second, third))
}

fn mul(left: Mat, right: Mat) -> Mat {
    Mat {
        a: left.a * right.a + left.c * right.b,
        b: left.b * right.a + left.d * right.b,
        c: left.a * right.c + left.c * right.d,
        d: left.b * right.c + left.d * right.d,
        e: left.a * right.e + left.c * right.f + left.e,
        f: left.b * right.e + left.d * right.f + left.f,
    }
}

fn apply(matrix: Mat, x: f64, y: f64) -> (f64, f64) {
    (
        matrix.a * x + matrix.c * y + matrix.e,
        matrix.b * x + matrix.d * y + matrix.f,
    )
}

impl App {
    pub(crate) async fn sync_asset_value(&self, asset_id: &str) -> Result<Option<Value>, DbError> {
        let row = self.asset_by_id(asset_id).await?;
        Ok(row.as_ref().map(sync_asset_json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_box_returns_to_the_original() {
        let edits = vec![EditAction {
            action: "crop".to_string(),
            parameters: json!({ "x": 10, "y": 0, "width": 10, "height": 10 }),
        }];
        let (x1, y1, x2, y2) = invert_face_box(
            PreviewBox {
                x: 0.0,
                y: 0.0,
                width: 4.0,
                height: 4.0,
                image_width: 10.0,
            },
            &edits,
            10.0,
            10.0,
        );
        assert_eq!((x1, y1, x2, y2), (10.0, 0.0, 14.0, 4.0));
    }
}
