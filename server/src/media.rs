//! Original upload, asset documents, trash, and stacks.
//! `libraryId` stays null. A duplicate checksum, including one in the trash, returns the existing Asset.

use base64::Engine;
use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};
use crate::thumbs::{self, ThumbFiles};

const ASSET_SQL: &str = "SELECT
    asset.id AS id,
    asset.ownerId AS owner_id,
    asset.type AS asset_type,
    asset.originalPath AS original_path,
    asset.originalFileName AS original_file_name,
    asset.originalMimeType AS original_mime_type,
    asset.fileCreatedAt AS file_created_at,
    asset.fileModifiedAt AS file_modified_at,
    asset.localDateTime AS local_date_time,
    asset.isFavorite AS is_favorite,
    asset.duration AS duration,
    asset.checksum AS checksum,
    asset.livePhotoVideoId AS live_photo_video_id,
    asset.thumbhash AS thumbhash,
    asset.deletedAt AS deleted_at,
    asset.stackId AS stack_id,
    asset.duplicateId AS duplicate_id,
    asset.visibility AS visibility,
    asset.width AS width,
    asset.height AS height,
    asset.isEdited AS is_edited,
    asset.isOffline AS is_offline,
    asset.createdAt AS created_at,
    asset.updatedAt AS updated_at,
    asset.updateId AS update_id,
    asset_exif.fileSizeInByte AS file_size,
    asset_exif.description AS description,
    asset_exif.make AS make,
    asset_exif.model AS model,
    asset_exif.orientation AS orientation,
    asset_exif.dateTimeOriginal AS date_time_original,
    asset_exif.modifyDate AS modify_date,
    asset_exif.timeZone AS time_zone,
    asset_exif.lensModel AS lens_model,
    asset_exif.fNumber AS f_number,
    asset_exif.focalLength AS focal_length,
    asset_exif.iso AS iso,
    asset_exif.exposureTime AS exposure_time,
    asset_exif.latitude AS latitude,
    asset_exif.longitude AS longitude,
    asset_exif.city AS city,
    asset_exif.state AS state,
    asset_exif.country AS country,
    asset_exif.projectionType AS projection_type,
    asset_exif.rating AS rating,
    asset_exif.exifImageWidth AS exif_width,
    asset_exif.exifImageHeight AS exif_height
FROM asset
LEFT JOIN asset_exif ON asset_exif.assetId = asset.id";

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct AssetRecord {
    pub id: String,
    pub owner_id: String,
    pub asset_type: String,
    pub original_path: String,
    pub original_file_name: String,
    pub original_mime_type: String,
    pub file_created_at: String,
    pub file_modified_at: String,
    pub local_date_time: String,
    pub is_favorite: i64,
    pub duration: Option<i64>,
    pub checksum: Vec<u8>,
    pub live_photo_video_id: Option<String>,
    pub thumbhash: Option<Vec<u8>>,
    pub deleted_at: Option<String>,
    pub stack_id: Option<String>,
    pub duplicate_id: Option<String>,
    pub visibility: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub is_edited: i64,
    pub is_offline: i64,
    pub created_at: String,
    pub updated_at: String,
    pub update_id: String,
    pub file_size: Option<i64>,
    pub description: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub orientation: Option<String>,
    pub date_time_original: Option<String>,
    pub modify_date: Option<String>,
    pub time_zone: Option<String>,
    pub lens_model: Option<String>,
    pub f_number: Option<f64>,
    pub focal_length: Option<f64>,
    pub iso: Option<i64>,
    pub exposure_time: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub projection_type: Option<String>,
    pub rating: Option<i64>,
    pub exif_width: Option<i64>,
    pub exif_height: Option<i64>,
}

pub struct IncomingOriginal {
    pub temp_path: std::path::PathBuf,
    pub checksum: Vec<u8>,
    pub byte_size: i64,
    pub filename: String,
    pub file_created_at: String,
    pub file_modified_at: String,
    pub is_favorite: bool,
    pub duration_ms: Option<i64>,
    pub visibility: String,
    pub live_photo_video_id: Option<String>,
    pub sidecar_temp: Option<std::path::PathBuf>,
}

pub enum IngestResult {
    Created(String),
    Duplicate(String),
}

pub fn classify_filename(filename: &str) -> (&'static str, &'static str) {
    let extension = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => ("IMAGE", "image/jpeg"),
        "png" => ("IMAGE", "image/png"),
        "gif" => ("IMAGE", "image/gif"),
        "webp" => ("IMAGE", "image/webp"),
        "heic" => ("IMAGE", "image/heic"),
        "heif" => ("IMAGE", "image/heif"),
        "dng" => ("IMAGE", "image/dng"),
        "tif" | "tiff" => ("IMAGE", "image/tiff"),
        "mp4" | "m4v" => ("VIDEO", "video/mp4"),
        "mov" => ("VIDEO", "video/quicktime"),
        "mkv" => ("VIDEO", "video/x-matroska"),
        "webm" => ("VIDEO", "video/webm"),
        "avi" => ("VIDEO", "video/x-msvideo"),
        "mp3" => ("AUDIO", "audio/mpeg"),
        "wav" => ("AUDIO", "audio/wav"),
        "m4a" => ("AUDIO", "audio/mp4"),
        _ => ("OTHER", "application/octet-stream"),
    }
}

pub fn checksum_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_checksum(value: &str) -> Option<Vec<u8>> {
    let bytes = if value.len() == 28 {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .ok()?
    } else {
        hex::decode(value).ok()?
    };
    if bytes.len() == 20 {
        Some(bytes)
    } else {
        None
    }
}

impl App {
    pub async fn ingest_original(
        &self,
        owner_id: &str,
        quota_size: Option<i64>,
        file: &IncomingOriginal,
    ) -> Result<IngestResult, DbError> {
        if let Some(limit) = quota_size {
            let usage = self.quota_usage(owner_id).await?;
            if usage.saturating_add(file.byte_size) > limit {
                return Err(DbError::Quota);
            }
        }
        if let Some(existing) = self.find_by_checksum(owner_id, &file.checksum).await? {
            return Ok(IngestResult::Duplicate(existing));
        }

        let asset_id = new_id();
        let key = format!("{owner_id}/{asset_id}/original");
        let (asset_type, mime) = classify_filename(&file.filename);
        let thumbs = generate_thumbs(self.data_dir(), &asset_id, &file.temp_path, asset_type).await;
        let sidecar_key = file.sidecar_temp.as_ref().map(|_| format!("{key}.xmp"));

        if let Err(error) = self.store().put_file(&key, &file.temp_path, mime).await {
            thumbs::remove_thumbs(self.data_dir(), &asset_id);
            return Err(error.into());
        }
        if let (Some(sidecar), Some(sidecar_key)) = (&file.sidecar_temp, &sidecar_key) {
            if let Err(error) = self
                .store()
                .put_file(sidecar_key, sidecar, "application/xml")
                .await
            {
                let _ = self.store().delete(&key).await;
                thumbs::remove_thumbs(self.data_dir(), &asset_id);
                return Err(error.into());
            }
        }

        let now = now_iso();
        let update_id = new_update_id();
        let width = thumbs.as_ref().map(|item| item.width);
        let height = thumbs.as_ref().map(|item| item.height);
        let insert = insert_asset(
            self,
            &asset_id,
            owner_id,
            asset_type,
            &key,
            &file.filename,
            mime,
            file,
            &update_id,
            &now,
            width,
            height,
            sidecar_key.as_deref(),
            thumbs.as_ref(),
        )
        .await;
        if let Err(error) = insert {
            let _ = self.store().delete(&key).await;
            if let Some(sidecar_key) = &sidecar_key {
                let _ = self.store().delete(sidecar_key).await;
            }
            thumbs::remove_thumbs(self.data_dir(), &asset_id);
            if is_unique(&error) {
                if let Some(existing) = self.find_by_checksum(owner_id, &file.checksum).await? {
                    return Ok(IngestResult::Duplicate(existing));
                }
            }
            return Err(error);
        }
        Ok(IngestResult::Created(asset_id))
    }

    pub async fn asset_json(&self, owner_id: &str, id: &str) -> Result<Value, DbError> {
        let record = self.asset_record(owner_id, id).await?;
        self.asset_value(&record).await
    }

    /// Asset document for a shared link. `strip` is the `showMetadata` false body.
    pub async fn share_asset_json(
        &self,
        owner_id: &str,
        id: &str,
        strip: bool,
    ) -> Result<Value, DbError> {
        let record = self.asset_record(owner_id, id).await?;
        if strip {
            return Ok(sanitized_asset(&record));
        }
        let mut value = self.asset_value(&record).await?;
        value["people"] = json!([]);
        Ok(value)
    }

    pub(crate) async fn asset_record(
        &self,
        owner_id: &str,
        id: &str,
    ) -> Result<AssetRecord, DbError> {
        let sql = format!("{ASSET_SQL} WHERE asset.id = ? AND asset.ownerId = ?");
        let record = sqlx::query_as::<_, AssetRecord>(&sql)
            .bind(id)
            .bind(owner_id)
            .fetch_optional(self.pool())
            .await?;
        record.ok_or(DbError::NotFound)
    }

    pub(crate) async fn changed_assets(
        &self,
        owner_id: &str,
        after: Option<&str>,
        before: &str,
    ) -> Result<Vec<AssetRecord>, DbError> {
        let sql = format!(
            "{ASSET_SQL} WHERE asset.ownerId = ? AND (? IS NULL OR asset.updateId > ?) AND asset.updateId < ? ORDER BY asset.updateId"
        );
        let rows = sqlx::query_as::<_, AssetRecord>(&sql)
            .bind(owner_id)
            .bind(after)
            .bind(after)
            .bind(before)
            .fetch_all(self.pool())
            .await?;
        Ok(rows)
    }

    pub(crate) async fn asset_value(&self, record: &AssetRecord) -> Result<Value, DbError> {
        let stack = if let Some(stack_id) = &record.stack_id {
            self.stack_summary(stack_id).await?
        } else {
            None
        };
        Ok(asset_json(record, stack))
    }

    pub async fn update_asset(
        &self,
        owner_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        self.apply_asset_update(owner_id, id, body).await?;
        self.asset_json(owner_id, id).await
    }

    pub async fn update_assets(&self, owner_id: &str, body: &Value) -> Result<(), DbError> {
        let ids = string_list(body.get("ids"))?;
        for id in ids {
            if self.asset_record(owner_id, &id).await.is_err() {
                continue;
            }
            self.apply_asset_update(owner_id, &id, body).await?;
        }
        Ok(())
    }

    pub async fn delete_assets(
        &self,
        owner_id: &str,
        ids: &[String],
        force: bool,
    ) -> Result<(), DbError> {
        for id in ids {
            if force {
                self.purge_asset(owner_id, id).await?;
            } else {
                sqlx::query(
                    "UPDATE asset SET deletedAt = ?, updatedAt = ?, updateId = ?
                     WHERE id = ? AND ownerId = ? AND deletedAt IS NULL",
                )
                .bind(now_iso())
                .bind(now_iso())
                .bind(new_update_id())
                .bind(id)
                .bind(owner_id)
                .execute(self.pool())
                .await?;
            }
        }
        Ok(())
    }

    pub async fn restore_assets(
        &self,
        owner_id: &str,
        ids: Option<&[String]>,
    ) -> Result<i64, DbError> {
        let now = now_iso();
        if let Some(ids) = ids {
            let mut count = 0;
            for id in ids {
                let result = sqlx::query(
                    "UPDATE asset SET deletedAt = NULL, updatedAt = ?, updateId = ?
                     WHERE id = ? AND ownerId = ? AND deletedAt IS NOT NULL",
                )
                .bind(&now)
                .bind(new_update_id())
                .bind(id)
                .bind(owner_id)
                .execute(self.pool())
                .await?;
                count += result.rows_affected() as i64;
            }
            return Ok(count);
        }
        let result = sqlx::query(
            "UPDATE asset SET deletedAt = NULL, updatedAt = ?, updateId = ?
             WHERE ownerId = ? AND deletedAt IS NOT NULL",
        )
        .bind(&now)
        .bind(new_update_id())
        .bind(owner_id)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() as i64)
    }

    pub async fn empty_trash(&self, owner_id: &str) -> Result<i64, DbError> {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM asset WHERE ownerId = ? AND deletedAt IS NOT NULL")
                .bind(owner_id)
                .fetch_all(self.pool())
                .await?;
        let count = ids.len() as i64;
        for id in ids {
            self.purge_asset(owner_id, &id).await?;
        }
        Ok(count)
    }

    pub async fn bulk_upload_check(&self, owner_id: &str, body: &Value) -> Result<Value, DbError> {
        let assets = body
            .get("assets")
            .and_then(|value| value.as_array())
            .ok_or(DbError::Validation)?;
        let mut results = Vec::new();
        for asset in assets {
            let id = asset
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let checksum = asset
                .get("checksum")
                .and_then(|value| value.as_str())
                .and_then(decode_checksum);
            let Some(checksum) = checksum else {
                results.push(json!({ "id": id, "action": "accept" }));
                continue;
            };
            let row: Option<(String, Option<String>)> = sqlx::query_as(
                "SELECT id, deletedAt FROM asset WHERE ownerId = ? AND checksum = ?",
            )
            .bind(owner_id)
            .bind(checksum.as_slice())
            .fetch_optional(self.pool())
            .await?;
            if let Some((asset_id, deleted_at)) = row {
                results.push(json!({
                    "id": id,
                    "action": "reject",
                    "reason": "duplicate",
                    "assetId": asset_id,
                    "isTrashed": deleted_at.is_some(),
                }));
            } else {
                results.push(json!({ "id": id, "action": "accept" }));
            }
        }
        Ok(json!({ "results": results }))
    }

    pub async fn run_asset_job(&self, owner_id: &str, body: &Value) -> Result<(), DbError> {
        let name = body
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let ids = string_list(body.get("assetIds"))?;
        // `transcode-video` is accepted and writes no encoded video.
        if name == "regenerate-thumbnail" {
            for id in ids {
                let Ok(record) = self.asset_record(owner_id, &id).await else {
                    continue;
                };
                let source = self.data_dir().join("objects").join(&record.original_path);
                if source.is_file() {
                    let _ =
                        generate_thumbs(self.data_dir(), &record.id, &source, &record.asset_type)
                            .await;
                }
            }
        }
        Ok(())
    }

    pub async fn create_stack(
        &self,
        owner_id: &str,
        asset_ids: &[String],
    ) -> Result<Value, DbError> {
        if asset_ids.len() < 2 {
            return Err(DbError::Validation);
        }
        let mut records = Vec::new();
        for id in asset_ids {
            records.push(self.asset_record(owner_id, id).await?);
        }
        let stack_id = new_id();
        let now = now_iso();
        let primary = &asset_ids[0];
        sqlx::query(
            "INSERT INTO stack (id, ownerId, primaryAssetId, createdAt, updatedAt, updateId)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&stack_id)
        .bind(owner_id)
        .bind(primary)
        .bind(&now)
        .bind(&now)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        for id in asset_ids {
            sqlx::query("UPDATE asset SET stackId = ?, updatedAt = ?, updateId = ? WHERE id = ? AND ownerId = ?")
                .bind(&stack_id)
                .bind(&now)
                .bind(new_update_id())
                .bind(id)
                .bind(owner_id)
                .execute(self.pool())
                .await?;
        }
        let mut assets = Vec::new();
        for id in asset_ids {
            assets.push(self.asset_json(owner_id, id).await?);
        }
        Ok(json!({
            "id": stack_id,
            "primaryAssetId": primary,
            "assets": assets,
        }))
    }

    pub async fn original_response(
        &self,
        owner_id: &str,
        id: &str,
        range_header: Option<&str>,
    ) -> Result<axum::response::Response, DbError> {
        let record = self.asset_record(owner_id, id).await?;
        let response = self
            .store()
            .bytes_response(
                &record.original_path,
                range_header,
                &record.original_mime_type,
                &record.original_file_name,
            )
            .await?;
        response.ok_or(DbError::NotFound)
    }

    pub async fn thumbnail_response(
        &self,
        owner_id: &str,
        id: &str,
        size: &str,
        range_header: Option<&str>,
    ) -> Result<axum::response::Response, DbError> {
        let record = self.asset_record(owner_id, id).await?;
        let file_type = if size == "preview" {
            "preview"
        } else {
            "thumbnail"
        };
        let relative: Option<String> =
            sqlx::query_scalar("SELECT path FROM asset_file WHERE assetId = ? AND type = ?")
                .bind(&record.id)
                .bind(file_type)
                .fetch_optional(self.pool())
                .await?;
        let relative = if let Some(path) = relative {
            path
        } else {
            let source = self.data_dir().join("objects").join(&record.original_path);
            if !source.is_file() {
                return Err(DbError::NotFound);
            }
            let thumbs =
                generate_thumbs(self.data_dir(), &record.id, &source, &record.asset_type).await;
            let Some(thumbs) = thumbs else {
                return Err(DbError::NotFound);
            };
            self.store_thumb_rows(&record.id, &thumbs).await?;
            if file_type == "preview" {
                format!("thumbs/{}/preview.jpg", record.id)
            } else {
                format!("thumbs/{}/thumbnail.jpg", record.id)
            }
        };
        let path = self.data_dir().join(&relative);
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| DbError::NotFound)?;
        Ok(jpeg_response(bytes, range_header))
    }

    async fn store_thumb_rows(&self, asset_id: &str, thumbs: &ThumbFiles) -> Result<(), DbError> {
        for (kind, relative) in [
            ("thumbnail", format!("thumbs/{asset_id}/thumbnail.jpg")),
            ("preview", format!("thumbs/{asset_id}/preview.jpg")),
        ] {
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
        sqlx::query(
            "UPDATE asset SET width = ?, height = ?, updatedAt = ?, updateId = ? WHERE id = ?",
        )
        .bind(thumbs.width)
        .bind(thumbs.height)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(asset_id)
        .execute(self.pool())
        .await?;
        let _ = thumbs;
        Ok(())
    }

    async fn apply_asset_update(
        &self,
        owner_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<(), DbError> {
        let mut record = self.asset_record(owner_id, id).await?;
        if body.get("isFavorite").is_some() {
            record.is_favorite =
                i64::from(body["isFavorite"].as_bool().ok_or(DbError::Validation)?);
        }
        if let Some(visibility) = body.get("visibility") {
            let visibility = visibility.as_str().ok_or(DbError::Validation)?;
            if !matches!(visibility, "timeline" | "archive" | "hidden" | "locked") {
                return Err(DbError::Validation);
            }
            record.visibility = visibility.to_string();
        }
        if body.get("livePhotoVideoId").is_some() {
            record.live_photo_video_id = body["livePhotoVideoId"].as_str().map(str::to_string);
        }
        let has_latitude = body.get("latitude").is_some();
        let has_longitude = body.get("longitude").is_some();
        if has_latitude != has_longitude {
            return Err(DbError::Validation);
        }
        if has_latitude {
            record.latitude = json_f64(body.get("latitude"));
            record.longitude = json_f64(body.get("longitude"));
        }
        if let Some(description) = body.get("description") {
            record.description = Some(description.as_str().unwrap_or("").to_string());
        }
        if let Some(rating) = body.get("rating") {
            if rating.is_null() {
                record.rating = None;
            } else {
                record.rating = Some(rating.as_i64().ok_or(DbError::Validation)?);
            }
        }
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "UPDATE asset SET isFavorite = ?, visibility = ?, livePhotoVideoId = ?, updatedAt = ?, updateId = ?
             WHERE id = ? AND ownerId = ?",
        )
        .bind(record.is_favorite)
        .bind(&record.visibility)
        .bind(&record.live_photo_video_id)
        .bind(&now)
        .bind(&update_id)
        .bind(id)
        .bind(owner_id)
        .execute(self.pool())
        .await?;
        sqlx::query(
            "UPDATE asset_exif SET description = ?, latitude = ?, longitude = ?, rating = ?, updatedAt = ?, updateId = ?
             WHERE assetId = ?",
        )
        .bind(record.description.as_deref().unwrap_or(""))
        .bind(record.latitude)
        .bind(record.longitude)
        .bind(record.rating)
        .bind(&now)
        .bind(new_update_id())
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn purge_asset(&self, owner_id: &str, id: &str) -> Result<bool, DbError> {
        let Some(record) = ({
            let sql = format!("{ASSET_SQL} WHERE asset.id = ? AND asset.ownerId = ?");
            sqlx::query_as::<_, AssetRecord>(&sql)
                .bind(id)
                .bind(owner_id)
                .fetch_optional(self.pool())
                .await?
        }) else {
            return Ok(false);
        };
        let links: Vec<String> =
            sqlx::query_scalar("SELECT albumId FROM album_asset WHERE assetId = ?")
                .bind(id)
                .fetch_all(self.pool())
                .await?;
        for album_id in &links {
            sqlx::query(
                "INSERT INTO album_asset_audit (id, albumId, assetId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(new_id())
            .bind(album_id)
            .bind(id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        }
        sqlx::query("DELETE FROM album_asset WHERE assetId = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        sqlx::query("INSERT INTO asset_audit (id, assetId, ownerId, updateId) VALUES (?, ?, ?, ?)")
            .bind(new_id())
            .bind(id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        sqlx::query("DELETE FROM asset WHERE id = ? AND ownerId = ?")
            .bind(id)
            .bind(owner_id)
            .execute(self.pool())
            .await?;
        let _ = self.store().delete(&record.original_path).await;
        let _ = self
            .store()
            .delete(&format!("{}.xmp", record.original_path))
            .await;
        thumbs::remove_thumbs(self.data_dir(), id);
        if let Some(size) = record.file_size {
            sqlx::query(
                "UPDATE user SET quotaUsageInBytes = MAX(quotaUsageInBytes - ?, 0) WHERE id = ?",
            )
            .bind(size)
            .bind(owner_id)
            .execute(self.pool())
            .await?;
        }
        Ok(true)
    }

    async fn find_by_checksum(
        &self,
        owner_id: &str,
        checksum: &[u8],
    ) -> Result<Option<String>, DbError> {
        let id: Option<String> =
            sqlx::query_scalar("SELECT id FROM asset WHERE ownerId = ? AND checksum = ?")
                .bind(owner_id)
                .bind(checksum)
                .fetch_optional(self.pool())
                .await?;
        Ok(id)
    }

    async fn quota_usage(&self, owner_id: &str) -> Result<i64, DbError> {
        let usage: i64 = sqlx::query_scalar("SELECT quotaUsageInBytes FROM user WHERE id = ?")
            .bind(owner_id)
            .fetch_one(self.pool())
            .await?;
        Ok(usage)
    }

    async fn stack_summary(&self, stack_id: &str) -> Result<Option<Value>, DbError> {
        let primary: Option<String> =
            sqlx::query_scalar("SELECT primaryAssetId FROM stack WHERE id = ?")
                .bind(stack_id)
                .fetch_optional(self.pool())
                .await?;
        let Some(primary) = primary else {
            return Ok(None);
        };
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset WHERE stackId = ?")
            .bind(stack_id)
            .fetch_one(self.pool())
            .await?;
        Ok(Some(json!({
            "id": stack_id,
            "primaryAssetId": primary,
            "assetCount": count,
        })))
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_asset(
    app: &App,
    asset_id: &str,
    owner_id: &str,
    asset_type: &str,
    key: &str,
    filename: &str,
    mime: &str,
    file: &IncomingOriginal,
    update_id: &str,
    now: &str,
    width: Option<i64>,
    height: Option<i64>,
    sidecar_key: Option<&str>,
    thumbs: Option<&ThumbFiles>,
) -> Result<(), DbError> {
    let mut transaction = app.pool().begin().await?;
    sqlx::query(
        "INSERT INTO asset (
            id, ownerId, type, originalPath, originalFileName, originalMimeType,
            fileCreatedAt, fileModifiedAt, localDateTime, isFavorite, duration,
            checksum, checksumAlgorithm, livePhotoVideoId, thumbhash, isOffline,
            libraryId, isExternal, deletedAt, stackId, duplicateId, status,
            updateId, visibility, width, height, isEdited, createdAt, updatedAt
        ) VALUES (
            ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, 'sha1', ?, NULL, 0,
            NULL, 0, NULL, NULL, NULL, 'active',
            ?, ?, ?, ?, 0, ?, ?
        )",
    )
    .bind(asset_id)
    .bind(owner_id)
    .bind(asset_type)
    .bind(key)
    .bind(filename)
    .bind(mime)
    .bind(&file.file_created_at)
    .bind(&file.file_modified_at)
    .bind(&file.file_created_at)
    .bind(i64::from(file.is_favorite))
    .bind(file.duration_ms)
    .bind(file.checksum.as_slice())
    .bind(&file.live_photo_video_id)
    .bind(update_id)
    .bind(&file.visibility)
    .bind(width)
    .bind(height)
    .bind(now)
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO asset_exif (
            assetId, fileSizeInByte, exifImageWidth, exifImageHeight, description, updatedAt, updateId
        ) VALUES (?, ?, ?, ?, '', ?, ?)",
    )
    .bind(asset_id)
    .bind(file.byte_size)
    .bind(width)
    .bind(height)
    .bind(now)
    .bind(update_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO asset_job_status (assetId, metadataExtractedAt) VALUES (?, ?)")
        .bind(asset_id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    if let Some(thumbs) = thumbs {
        for (kind, relative) in [
            ("thumbnail", format!("thumbs/{asset_id}/thumbnail.jpg")),
            ("preview", format!("thumbs/{asset_id}/preview.jpg")),
        ] {
            let _ = thumbs;
            sqlx::query("INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, ?, ?)")
                .bind(new_id())
                .bind(asset_id)
                .bind(kind)
                .bind(relative)
                .execute(&mut *transaction)
                .await?;
        }
    }
    if let Some(sidecar_key) = sidecar_key {
        sqlx::query("INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, 'sidecar', ?)")
            .bind(new_id())
            .bind(asset_id)
            .bind(sidecar_key)
            .execute(&mut *transaction)
            .await?;
    }
    sqlx::query("UPDATE user SET quotaUsageInBytes = quotaUsageInBytes + ? WHERE id = ?")
        .bind(file.byte_size)
        .bind(owner_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn generate_thumbs(
    data_dir: &std::path::Path,
    asset_id: &str,
    source: &std::path::Path,
    asset_type: &str,
) -> Option<ThumbFiles> {
    let data_dir = data_dir.to_path_buf();
    let asset_id = asset_id.to_string();
    let source = source.to_path_buf();
    let asset_type = asset_type.to_string();
    tokio::task::spawn_blocking(move || {
        if asset_type == "IMAGE" {
            thumbs::write_image_thumbs(&data_dir, &asset_id, &source)
        } else if asset_type == "VIDEO" {
            thumbs::write_video_frame(&data_dir, &asset_id, &source)
        } else {
            None
        }
    })
    .await
    .ok()
    .flatten()
}

pub(crate) fn sanitized_asset(record: &AssetRecord) -> Value {
    json!({
        "id": record.id,
        "type": record.asset_type,
        "originalMimeType": record.original_mime_type,
        "thumbhash": record.thumbhash.as_ref().map(|bytes| checksum_base64(bytes)),
        "localDateTime": record.local_date_time,
        "duration": record.duration,
        "livePhotoVideoId": record.live_photo_video_id,
        "hasMetadata": false,
        "width": record.width,
        "height": record.height,
    })
}

fn jpeg_response(bytes: Vec<u8>, range_header: Option<&str>) -> axum::response::Response {
    use axum::http::{header, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    const CACHE: &str = "private, max-age=86400, no-transform, stale-while-revalidate=2592000, stale-if-error=2592000";
    let len = bytes.len() as u64;
    if let Some(header_value) = range_header {
        let Some((start, end)) = crate::store::parse_range(header_value, len) else {
            let mut response = (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{len}")).expect("range"),
                )],
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE));
            return response;
        };
        let slice = bytes[start as usize..=end as usize].to_vec();
        let slice_len = slice.len();
        let mut response = (
            StatusCode::PARTIAL_CONTENT,
            [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"))],
            slice,
        )
            .into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&slice_len.to_string()).expect("length"),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE));
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{len}")).expect("range"),
        );
        return response;
    }
    let byte_len = bytes.len();
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"))],
        bytes,
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&byte_len.to_string()).expect("length"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE));
    response
}

fn asset_json(record: &AssetRecord, stack: Option<Value>) -> Value {
    let mut value = json!({
        "id": record.id,
        "type": record.asset_type,
        "thumbhash": record.thumbhash.as_ref().map(|bytes| checksum_base64(bytes)),
        "originalMimeType": record.original_mime_type,
        "localDateTime": record.local_date_time,
        "duration": record.duration,
        "livePhotoVideoId": record.live_photo_video_id,
        "hasMetadata": true,
        "width": record.width,
        "height": record.height,
        "createdAt": record.created_at,
        "ownerId": record.owner_id,
        "libraryId": null,
        "originalPath": record.original_path,
        "originalFileName": record.original_file_name,
        "fileCreatedAt": record.file_created_at,
        "fileModifiedAt": record.file_modified_at,
        "updatedAt": record.updated_at,
        "isFavorite": record.is_favorite != 0,
        "isArchived": record.visibility == "archive",
        "isTrashed": record.deleted_at.is_some(),
        "isOffline": record.is_offline != 0,
        "visibility": record.visibility,
        "checksum": checksum_base64(&record.checksum),
        "isEdited": record.is_edited != 0,
        "duplicateId": record.duplicate_id,
        "stack": stack,
    });
    value["exifInfo"] = exif_json(record);
    value
}

fn exif_json(record: &AssetRecord) -> Value {
    json!({
        "make": record.make,
        "model": record.model,
        "exifImageWidth": record.exif_width.or(record.width),
        "exifImageHeight": record.exif_height.or(record.height),
        "fileSizeInByte": record.file_size,
        "orientation": record.orientation,
        "dateTimeOriginal": record.date_time_original,
        "modifyDate": record.modify_date,
        "timeZone": record.time_zone,
        "lensModel": record.lens_model,
        "fNumber": record.f_number,
        "focalLength": record.focal_length,
        "iso": record.iso,
        "exposureTime": record.exposure_time,
        "latitude": record.latitude,
        "longitude": record.longitude,
        "city": record.city,
        "state": record.state,
        "country": record.country,
        "description": record.description.clone().unwrap_or_default(),
        "projectionType": record.projection_type,
        "rating": record.rating,
    })
}

pub(crate) fn sync_asset_json(record: &AssetRecord) -> Value {
    json!({
        "id": record.id,
        "ownerId": record.owner_id,
        "originalFileName": record.original_file_name,
        "thumbhash": record.thumbhash.as_ref().map(|bytes| checksum_base64(bytes)),
        "checksum": checksum_base64(&record.checksum),
        "fileCreatedAt": record.file_created_at,
        "fileModifiedAt": record.file_modified_at,
        "createdAt": record.created_at,
        "localDateTime": record.local_date_time,
        "duration": record.duration,
        "type": record.asset_type,
        "deletedAt": record.deleted_at,
        "isFavorite": record.is_favorite != 0,
        "visibility": record.visibility,
        "livePhotoVideoId": record.live_photo_video_id,
        "stackId": record.stack_id,
        "libraryId": null,
        "width": record.width,
        "height": record.height,
        "isEdited": record.is_edited != 0,
    })
}

pub(crate) fn sync_exif_json(record: &AssetRecord) -> Value {
    json!({
        "assetId": record.id,
        "description": record.description.clone().unwrap_or_default(),
        "exifImageWidth": record.exif_width.or(record.width),
        "exifImageHeight": record.exif_height.or(record.height),
        "fileSizeInByte": record.file_size,
        "orientation": record.orientation,
        "dateTimeOriginal": record.date_time_original,
        "modifyDate": record.modify_date,
        "timeZone": record.time_zone,
        "latitude": record.latitude,
        "longitude": record.longitude,
        "projectionType": record.projection_type,
        "city": record.city,
        "state": record.state,
        "country": record.country,
        "make": record.make,
        "model": record.model,
        "lensModel": record.lens_model,
        "fNumber": record.f_number,
        "focalLength": record.focal_length,
        "iso": record.iso,
        "exposureTime": record.exposure_time,
        "profileDescription": null,
        "rating": record.rating,
        "fps": null,
    })
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>, DbError> {
    let list = value
        .and_then(|item| item.as_array())
        .ok_or(DbError::Validation)?;
    let mut ids = Vec::new();
    for item in list {
        ids.push(item.as_str().ok_or(DbError::Validation)?.to_string());
    }
    Ok(ids)
}

fn json_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(|item| {
        item.as_f64()
            .or_else(|| item.as_i64().map(|number| number as f64))
    })
}

fn is_unique(error: &DbError) -> bool {
    let DbError::Database(sqlx::Error::Database(database)) = error else {
        return false;
    };
    database
        .code()
        .as_deref()
        .is_some_and(|code| code == "2067" || code == "1555")
        || database.message().contains("UNIQUE constraint failed")
}
