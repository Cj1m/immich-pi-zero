//! Duplicate groups share `asset.duplicateId`. There is no duplicate table.
//! Search is exact cosine on the stored CLIP blob, nearest 64, at or under `maxDistance`.

use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};
use crate::ml::{bytes_to_floats, cosine_distance};

const SEARCH_LIMIT: usize = 64;

#[derive(sqlx::FromRow)]
struct DetectedAsset {
    id: String,
    owner_id: String,
    asset_type: String,
    duplicate_id: Option<String>,
    stack_id: Option<String>,
    visibility: String,
    deleted_at: Option<String>,
    embedding: Option<Vec<u8>>,
    detected_at: Option<String>,
}

struct Hit {
    id: String,
    duplicate_id: Option<String>,
    distance: f32,
}

struct ResolveGroup {
    duplicate_id: String,
    keep: Vec<String>,
    trash: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct GroupAsset {
    id: String,
    owner_id: String,
    is_favorite: i64,
    visibility: String,
    description: Option<String>,
    rating: Option<i64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

struct MergeFields {
    is_favorite: bool,
    visibility: String,
    description: Option<String>,
    rating: Option<i64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct SidecarSource {
    original_path: String,
    description: String,
    date_time_original: Option<String>,
    time_zone: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    rating: Option<i64>,
    tags: Option<String>,
    locked_properties: Option<String>,
}

impl App {
    /// Run duplicate detection for one Asset. A failure here must not fail an upload.
    pub(crate) async fn detect_duplicates(
        &self,
        asset_id: &str,
        force: bool,
    ) -> Result<(), DbError> {
        let Some(asset) = self.detected_asset(asset_id).await? else {
            return Ok(());
        };
        if !self.duplicate_detection_enabled().await? {
            return Ok(());
        }
        if asset.deleted_at.is_some() || asset.stack_id.is_some() {
            return Ok(());
        }
        if asset.visibility != "timeline" && asset.visibility != "archive" {
            return Ok(());
        }
        let Some(embedding) = asset.embedding.as_deref().and_then(bytes_to_floats) else {
            eprintln!("duplicate detection: asset {asset_id} is missing embedding");
            return Ok(());
        };
        if embedding.is_empty() {
            eprintln!("duplicate detection: asset {asset_id} is missing embedding");
            return Ok(());
        }
        if !force && asset.detected_at.is_some() {
            return Ok(());
        }
        let max_distance = self.duplicate_max_distance().await?;
        let hits = self
            .nearest_duplicates(&asset, &embedding, max_distance)
            .await?;
        let mut stamp = vec![asset.id.clone()];
        if hits.is_empty() {
            if asset.duplicate_id.is_some() {
                sqlx::query(
                    "UPDATE asset SET duplicateId = NULL, updatedAt = ?, updateId = ? WHERE id = ?",
                )
                .bind(now_iso())
                .bind(new_update_id())
                .bind(&asset.id)
                .execute(self.pool())
                .await?;
            }
        } else {
            let (target, source_ids, asset_ids) = target_group(&asset, &hits);
            self.assign_duplicate(&asset.owner_id, &target, &asset_ids, &source_ids)
                .await?;
            stamp = asset_ids;
        }
        self.stamp_detected(&stamp).await
    }

    pub async fn list_duplicates(&self, owner_id: &str) -> Result<Value, DbError> {
        self.collapse_singleton_groups(owner_id).await?;
        let group_ids: Vec<String> = sqlx::query_scalar(
            "SELECT duplicateId FROM asset
             WHERE ownerId = ? AND duplicateId IS NOT NULL AND deletedAt IS NULL
               AND stackId IS NULL AND visibility IN ('timeline', 'archive')
             GROUP BY duplicateId
             HAVING COUNT(*) > 1
             ORDER BY duplicateId",
        )
        .bind(owner_id)
        .fetch_all(self.pool())
        .await?;
        let mut items = Vec::new();
        for duplicate_id in group_ids {
            let asset_ids = self.visible_member_ids(owner_id, &duplicate_id).await?;
            if asset_ids.len() < 2 {
                continue;
            }
            let mut assets = Vec::new();
            for asset_id in &asset_ids {
                assets.push(self.asset_json(owner_id, asset_id).await?);
            }
            items.push(json!({
                "duplicateId": duplicate_id,
                "assets": assets,
                "suggestedKeepAssetIds": suggested_keep(&assets),
            }));
        }
        Ok(Value::Array(items))
    }

    pub async fn delete_duplicate(&self, owner_id: &str, id: &str) -> Result<(), DbError> {
        if !is_uuid_v4(id) {
            return Err(DbError::Validation);
        }
        self.require_owned_duplicates(owner_id, &[id.to_string()])
            .await?;
        self.clear_duplicate_ids(owner_id, &[id.to_string()]).await
    }

    pub async fn delete_duplicates(&self, owner_id: &str, body: &Value) -> Result<(), DbError> {
        let ids = uuid_list(body.get("ids"))?;
        self.require_owned_duplicates(owner_id, &ids).await?;
        self.clear_duplicate_ids(owner_id, &ids).await
    }

    pub async fn resolve_duplicates(&self, owner_id: &str, body: &Value) -> Result<Value, DbError> {
        let groups = parse_groups(body)?;
        let ids: Vec<String> = groups
            .iter()
            .map(|group| group.duplicate_id.clone())
            .collect();
        self.require_owned_duplicates(owner_id, &ids).await?;
        let mut results = Vec::new();
        for group in &groups {
            match self.resolve_one(owner_id, group).await {
                Ok(result) => results.push(result),
                Err(error) => {
                    eprintln!("duplicate resolve: {error:?}");
                    results.push(json!({
                        "id": group.duplicate_id,
                        "success": false,
                        "error": "unknown",
                    }));
                }
            }
        }
        Ok(Value::Array(results))
    }

    /// Write the XMP sidecar for the locked exif fields. Nothing locked skips the write.
    pub(crate) async fn write_locked_sidecar(&self, asset_id: &str) -> Result<(), DbError> {
        let source = sqlx::query_as::<_, SidecarSource>(
            "SELECT asset.originalPath AS original_path,
                    asset_exif.description AS description,
                    asset_exif.dateTimeOriginal AS date_time_original,
                    asset_exif.timeZone AS time_zone,
                    asset_exif.latitude AS latitude,
                    asset_exif.longitude AS longitude,
                    asset_exif.rating AS rating,
                    asset_exif.tags AS tags,
                    asset_exif.lockedProperties AS locked_properties
             FROM asset
             JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.id = ?",
        )
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(source) = source else {
            return Ok(());
        };
        let locked = locked_list(source.locked_properties.as_deref());
        let Some(document) = xmp_document(&source, &locked) else {
            return Ok(());
        };
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT path FROM asset_file WHERE assetId = ? AND type = 'sidecar'",
        )
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        let key = existing.unwrap_or_else(|| format!("{}.xmp", source.original_path));
        let temp = self
            .data_dir()
            .join("tmp")
            .join(format!("{}.xmp", new_id()));
        if let Some(parent) = temp.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| DbError::Store(error.to_string()))?;
        }
        tokio::fs::write(&temp, document)
            .await
            .map_err(|error| DbError::Store(error.to_string()))?;
        let stored = self.store().put_file(&key, &temp, "application/xml").await;
        let _ = tokio::fs::remove_file(&temp).await;
        stored?;
        sqlx::query(
            "INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, 'sidecar', ?)
             ON CONFLICT(assetId, type) DO UPDATE SET path = excluded.path",
        )
        .bind(new_id())
        .bind(asset_id)
        .bind(&key)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn resolve_one(&self, owner_id: &str, group: &ResolveGroup) -> Result<Value, DbError> {
        let assets = self.group_assets(&group.duplicate_id).await?;
        if assets.is_empty() {
            return Ok(json!({
                "id": group.duplicate_id,
                "success": false,
                "error": "not_found",
            }));
        }
        let member_ids: Vec<&str> = assets.iter().map(|asset| asset.id.as_str()).collect();
        let keep = members_of(&group.keep, &member_ids);
        let trash = members_of(&group.trash, &member_ids);
        for id in &member_ids {
            let in_keep = keep.iter().any(|item| item == id);
            let in_trash = trash.iter().any(|item| item == id);
            if in_keep && in_trash {
                return Ok(json!({
                    "id": group.duplicate_id,
                    "success": false,
                    "error": "validation",
                    "errorMessage": "An asset cannot be in both keepAssetIds and trashAssetIds",
                }));
            }
            if !in_keep && !in_trash {
                return Ok(json!({
                    "id": group.duplicate_id,
                    "success": false,
                    "error": "validation",
                    "errorMessage": "Every asset must be in either keepAssetIds or trashAssetIds",
                }));
            }
        }
        if !trash.is_empty() && !self.can_delete(owner_id, &trash).await? {
            return Ok(json!({
                "id": group.duplicate_id,
                "success": false,
                "error": "no_permission",
                "errorMessage": "No permission to delete assets",
            }));
        }

        let trash_on = self.trash_enabled().await?;
        let merge = keep.len() == 1 && !trash.is_empty();
        let fields = merge_fields(&assets);
        let mut exif_changed = false;
        let mut sidecar_id = None;
        let mut tx = self.pool().begin().await?;
        if merge {
            let keeper = keep[0];
            sidecar_id = Some(keeper.to_string());
            let keeper_asset = assets.iter().find(|asset| asset.id == keeper);
            let can_add = keeper_asset
                .is_some_and(|asset| asset.owner_id == owner_id && asset.visibility != "locked");
            if can_add {
                self.add_keeper_to_albums(&mut tx, owner_id, keeper, &member_ids)
                    .await?;
            }
            exif_changed = self.apply_merge_exif(&mut tx, keeper, &fields).await?;
            sqlx::query(
                "UPDATE asset
                 SET isFavorite = ?, visibility = ?, duplicateId = NULL, updatedAt = ?, updateId = ?
                 WHERE id = ? AND ownerId = ?",
            )
            .bind(i64::from(fields.is_favorite))
            .bind(&fields.visibility)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(keeper)
            .bind(owner_id)
            .execute(&mut *tx)
            .await?;
        } else if !keep.is_empty() {
            for id in &keep {
                sqlx::query(
                    "UPDATE asset SET duplicateId = NULL, updatedAt = ?, updateId = ?
                     WHERE id = ? AND ownerId = ?",
                )
                .bind(now_iso())
                .bind(new_update_id())
                .bind(id)
                .bind(owner_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        if trash_on {
            for id in &trash {
                sqlx::query(
                    "UPDATE asset
                     SET deletedAt = ?, status = 'trashed', duplicateId = NULL, updatedAt = ?, updateId = ?
                     WHERE id = ? AND ownerId = ?",
                )
                .bind(now_iso())
                .bind(now_iso())
                .bind(new_update_id())
                .bind(id)
                .bind(owner_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        if !trash_on {
            for id in &trash {
                self.purge_asset(owner_id, id).await?;
            }
        }
        let tags_changed = if merge {
            self.merge_group_tags(owner_id, keep[0], &member_ids)
                .await?
        } else {
            false
        };
        if exif_changed || tags_changed {
            if let Some(asset_id) = sidecar_id {
                if let Err(error) = self.write_locked_sidecar(&asset_id).await {
                    eprintln!("sidecar write: {error:?}");
                }
            }
        }
        Ok(json!({ "id": group.duplicate_id, "success": true }))
    }

    async fn detected_asset(&self, asset_id: &str) -> Result<Option<DetectedAsset>, DbError> {
        let asset = sqlx::query_as::<_, DetectedAsset>(
            "SELECT asset.id AS id, asset.ownerId AS owner_id, asset.type AS asset_type,
                    asset.duplicateId AS duplicate_id, asset.stackId AS stack_id,
                    asset.visibility AS visibility, asset.deletedAt AS deleted_at,
                    asset_clip.embedding AS embedding,
                    asset_job_status.duplicatesDetectedAt AS detected_at
             FROM asset
             LEFT JOIN asset_clip ON asset_clip.assetId = asset.id
             LEFT JOIN asset_job_status ON asset_job_status.assetId = asset.id
             WHERE asset.id = ?",
        )
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(asset)
    }

    async fn nearest_duplicates(
        &self,
        asset: &DetectedAsset,
        embedding: &[f32],
        max_distance: f64,
    ) -> Result<Vec<Hit>, DbError> {
        let rows: Vec<(String, Option<String>, Vec<u8>)> = sqlx::query_as(
            "SELECT asset.id, asset.duplicateId, asset_clip.embedding
             FROM asset
             JOIN asset_clip ON asset_clip.assetId = asset.id
             WHERE asset.ownerId = ?
               AND asset.deletedAt IS NULL
               AND asset.type = ?
               AND asset.id != ?
               AND asset.stackId IS NULL
               AND asset.visibility IN ('timeline', 'archive')",
        )
        .bind(&asset.owner_id)
        .bind(&asset.asset_type)
        .bind(&asset.id)
        .fetch_all(self.pool())
        .await?;
        let mut hits = Vec::new();
        for (id, duplicate_id, blob) in rows {
            let Some(other) = bytes_to_floats(&blob) else {
                continue;
            };
            if other.len() != embedding.len() {
                continue;
            }
            hits.push(Hit {
                id,
                duplicate_id,
                distance: cosine_distance(embedding, &other),
            });
        }
        hits.sort_by(|left, right| {
            left.distance
                .total_cmp(&right.distance)
                .then_with(|| left.id.cmp(&right.id))
        });
        hits.truncate(SEARCH_LIMIT);
        hits.retain(|hit| f64::from(hit.distance) <= max_distance);
        Ok(hits)
    }

    async fn assign_duplicate(
        &self,
        owner_id: &str,
        target: &str,
        asset_ids: &[String],
        source_ids: &[String],
    ) -> Result<(), DbError> {
        for id in asset_ids {
            sqlx::query(
                "UPDATE asset SET duplicateId = ?, updatedAt = ?, updateId = ?
                 WHERE ownerId = ? AND id = ?",
            )
            .bind(target)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(owner_id)
            .bind(id)
            .execute(self.pool())
            .await?;
        }
        for source in source_ids {
            if source == target {
                continue;
            }
            sqlx::query(
                "UPDATE asset SET duplicateId = ?, updatedAt = ?, updateId = ?
                 WHERE ownerId = ? AND duplicateId = ?",
            )
            .bind(target)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(owner_id)
            .bind(source)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    async fn stamp_detected(&self, ids: &[String]) -> Result<(), DbError> {
        let now = now_iso();
        for id in ids {
            sqlx::query(
                "INSERT INTO asset_job_status (assetId, duplicatesDetectedAt) VALUES (?, ?)
                 ON CONFLICT(assetId) DO UPDATE SET duplicatesDetectedAt = excluded.duplicatesDetectedAt",
            )
            .bind(id)
            .bind(&now)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    async fn collapse_singleton_groups(&self, owner_id: &str) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE asset SET duplicateId = NULL
             WHERE ownerId = ?
               AND duplicateId IN (
                 SELECT duplicateId FROM (
                   SELECT duplicateId FROM asset
                   WHERE ownerId = ? AND duplicateId IS NOT NULL
                     AND deletedAt IS NULL AND stackId IS NULL
                   GROUP BY duplicateId
                   HAVING COUNT(*) = 1
                 )
               )",
        )
        .bind(owner_id)
        .bind(owner_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn visible_member_ids(
        &self,
        owner_id: &str,
        duplicate_id: &str,
    ) -> Result<Vec<String>, DbError> {
        let ids = sqlx::query_scalar(
            "SELECT id FROM asset
             WHERE ownerId = ? AND duplicateId = ? AND deletedAt IS NULL AND stackId IS NULL
               AND visibility IN ('timeline', 'archive')
             ORDER BY localDateTime, id",
        )
        .bind(owner_id)
        .bind(duplicate_id)
        .fetch_all(self.pool())
        .await?;
        Ok(ids)
    }

    async fn group_assets(&self, duplicate_id: &str) -> Result<Vec<GroupAsset>, DbError> {
        let assets = sqlx::query_as::<_, GroupAsset>(
            "SELECT asset.id AS id, asset.ownerId AS owner_id, asset.isFavorite AS is_favorite,
                    asset.visibility AS visibility, asset_exif.description AS description,
                    asset_exif.rating AS rating, asset_exif.latitude AS latitude,
                    asset_exif.longitude AS longitude
             FROM asset
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.duplicateId = ? AND asset.deletedAt IS NULL AND asset.stackId IS NULL
               AND asset.visibility IN ('timeline', 'archive')
             ORDER BY asset.localDateTime, asset.id",
        )
        .bind(duplicate_id)
        .fetch_all(self.pool())
        .await?;
        Ok(assets)
    }

    async fn require_owned_duplicates(
        &self,
        owner_id: &str,
        ids: &[String],
    ) -> Result<(), DbError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut unique = Vec::new();
        for id in ids {
            if !unique.iter().any(|item: &String| item == id) {
                unique.push(id.clone());
            }
        }
        let owned = self.owned_duplicate_ids(owner_id, &unique).await?;
        if owned.len() != unique.len() {
            return Err(DbError::BadRequest(
                "Not found or no duplicate.delete access",
            ));
        }
        Ok(())
    }

    async fn owned_duplicate_ids(
        &self,
        owner_id: &str,
        ids: &[String],
    ) -> Result<Vec<String>, DbError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT duplicateId FROM asset
             WHERE ownerId = ? AND deletedAt IS NULL AND duplicateId IN ({placeholders})"
        );
        let mut query = sqlx::query_scalar(&sql).bind(owner_id);
        for id in ids {
            query = query.bind(id);
        }
        Ok(query.fetch_all(self.pool()).await?)
    }

    async fn clear_duplicate_ids(&self, owner_id: &str, ids: &[String]) -> Result<(), DbError> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE asset SET duplicateId = NULL, updatedAt = ?, updateId = ?
             WHERE ownerId = ? AND duplicateId IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(owner_id);
        for id in ids {
            query = query.bind(id);
        }
        query.execute(self.pool()).await?;
        Ok(())
    }

    async fn can_delete(&self, owner_id: &str, ids: &[&str]) -> Result<bool, DbError> {
        if ids.is_empty() {
            return Ok(true);
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id FROM asset
             WHERE ownerId = ? AND visibility != 'locked' AND id IN ({placeholders})"
        );
        let mut query = sqlx::query_scalar::<_, String>(&sql).bind(owner_id);
        for id in ids {
            query = query.bind(id);
        }
        let allowed = query.fetch_all(self.pool()).await?;
        Ok(allowed.len() == ids.len())
    }

    async fn add_keeper_to_albums(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        owner_id: &str,
        keeper: &str,
        asset_ids: &[&str],
    ) -> Result<(), DbError> {
        if asset_ids.is_empty() {
            return Ok(());
        }
        let placeholders = std::iter::repeat_n("?", asset_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT album.id
             FROM album
             JOIN album_asset ON album_asset.albumId = album.id
             JOIN album_user ON album_user.albumId = album.id
             WHERE album_user.userId = ?
               AND album_user.role IN ('owner', 'editor')
               AND album.deletedAt IS NULL
               AND album_asset.assetId IN ({placeholders})"
        );
        let mut query = sqlx::query_scalar::<_, String>(&sql).bind(owner_id);
        for id in asset_ids {
            query = query.bind(id);
        }
        let albums = query.fetch_all(&mut **tx).await?;
        let now = now_iso();
        for album_id in albums {
            let inserted = sqlx::query(
                "INSERT INTO album_asset (albumId, assetId, createdAt, updateId)
                 SELECT ?, ?, ?, ?
                 WHERE NOT EXISTS (
                   SELECT 1 FROM album_asset WHERE albumId = ? AND assetId = ?
                 )",
            )
            .bind(&album_id)
            .bind(keeper)
            .bind(&now)
            .bind(new_update_id())
            .bind(&album_id)
            .bind(keeper)
            .execute(&mut **tx)
            .await?;
            if inserted.rows_affected() == 0 {
                continue;
            }
            sqlx::query(
                "UPDATE album
                 SET albumThumbnailAssetId = COALESCE(albumThumbnailAssetId, ?),
                     updatedAt = ?, updateId = ?
                 WHERE id = ?",
            )
            .bind(keeper)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(&album_id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    async fn apply_merge_exif(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        keeper: &str,
        fields: &MergeFields,
    ) -> Result<bool, DbError> {
        let mut locked = self.locked_names(tx, keeper).await?;
        let mut changed = false;
        let now = now_iso();
        if let Some(description) = &fields.description {
            sqlx::query(
                "UPDATE asset_exif SET description = ?, updatedAt = ?, updateId = ? WHERE assetId = ?",
            )
            .bind(description)
            .bind(&now)
            .bind(new_update_id())
            .bind(keeper)
            .execute(&mut **tx)
            .await?;
            remember(&mut locked, "description");
            changed = true;
        }
        if let Some(rating) = fields.rating {
            sqlx::query(
                "UPDATE asset_exif SET rating = ?, updatedAt = ?, updateId = ? WHERE assetId = ?",
            )
            .bind(rating)
            .bind(&now)
            .bind(new_update_id())
            .bind(keeper)
            .execute(&mut **tx)
            .await?;
            remember(&mut locked, "rating");
            changed = true;
        }
        if let (Some(latitude), Some(longitude)) = (fields.latitude, fields.longitude) {
            sqlx::query(
                "UPDATE asset_exif
                 SET latitude = ?, longitude = ?, updatedAt = ?, updateId = ?
                 WHERE assetId = ?",
            )
            .bind(latitude)
            .bind(longitude)
            .bind(&now)
            .bind(new_update_id())
            .bind(keeper)
            .execute(&mut **tx)
            .await?;
            remember(&mut locked, "latitude");
            remember(&mut locked, "longitude");
            changed = true;
        }
        if changed {
            sqlx::query("UPDATE asset_exif SET lockedProperties = ? WHERE assetId = ?")
                .bind(serde_json::to_string(&locked).unwrap_or_else(|_| "[]".to_string()))
                .bind(keeper)
                .execute(&mut **tx)
                .await?;
        }
        Ok(changed)
    }

    async fn locked_names(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        asset_id: &str,
    ) -> Result<Vec<String>, DbError> {
        let text = sqlx::query_scalar::<_, Option<String>>(
            "SELECT lockedProperties FROM asset_exif WHERE assetId = ?",
        )
        .bind(asset_id)
        .fetch_optional(&mut **tx)
        .await?;
        Ok(locked_list(text.flatten().as_deref()))
    }

    async fn duplicate_detection_enabled(&self) -> Result<bool, DbError> {
        let config = self.admin_config().await?;
        let machine_learning = &config["machineLearning"];
        Ok(machine_learning["enabled"].as_bool().unwrap_or(false)
            && machine_learning["clip"]["enabled"]
                .as_bool()
                .unwrap_or(false)
            && machine_learning["duplicateDetection"]["enabled"]
                .as_bool()
                .unwrap_or(false))
    }

    async fn duplicate_max_distance(&self) -> Result<f64, DbError> {
        let config = self.admin_config().await?;
        Ok(
            json_f64(&config["machineLearning"]["duplicateDetection"]["maxDistance"])
                .unwrap_or(0.01),
        )
    }

    async fn trash_enabled(&self) -> Result<bool, DbError> {
        let config = self.admin_config().await?;
        Ok(config["trash"]["enabled"].as_bool().unwrap_or(true))
    }
}

fn target_group(asset: &DetectedAsset, hits: &[Hit]) -> (String, Vec<String>, Vec<String>) {
    let mut source_ids = Vec::new();
    for hit in hits {
        if let Some(id) = &hit.duplicate_id {
            if !source_ids.iter().any(|existing| existing == id) {
                source_ids.push(id.clone());
            }
        }
    }
    let target = if let Some(id) = &asset.duplicate_id {
        id.clone()
    } else if !source_ids.is_empty() {
        source_ids.remove(0)
    } else {
        new_id()
    };
    let mut asset_ids: Vec<String> = hits
        .iter()
        .filter(|hit| hit.duplicate_id.as_deref() != Some(target.as_str()))
        .map(|hit| hit.id.clone())
        .collect();
    asset_ids.push(asset.id.clone());
    (target, source_ids, asset_ids)
}

fn merge_fields(assets: &[GroupAsset]) -> MergeFields {
    let is_favorite = assets.iter().any(|asset| asset.is_favorite != 0);
    let visibility = ["locked", "archive", "timeline"]
        .into_iter()
        .find(|level| assets.iter().any(|asset| asset.visibility == *level))
        .or_else(|| {
            assets
                .iter()
                .any(|asset| asset.visibility == "hidden")
                .then_some("hidden")
        })
        .unwrap_or("timeline")
        .to_string();
    let mut rating = 0;
    for asset in assets {
        if let Some(value) = asset.rating {
            if value > rating {
                rating = value;
            }
        }
    }
    let description = unique_lines(assets.iter().map(|asset| asset.description.as_deref()));
    let latitude = unique_coordinate(assets.iter().filter_map(|asset| asset.latitude));
    let longitude = unique_coordinate(assets.iter().filter_map(|asset| asset.longitude));
    let (latitude, longitude) = match (latitude, longitude) {
        (Some(latitude), Some(longitude)) => (Some(latitude), Some(longitude)),
        _ => (None, None),
    };
    MergeFields {
        is_favorite,
        visibility,
        description,
        rating: (rating > 0).then_some(rating),
        latitude,
        longitude,
    }
}

fn unique_lines<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    let mut seen = Vec::new();
    let mut lines = Vec::new();
    for value in values.flatten() {
        if value.is_empty() {
            continue;
        }
        for line in value.split('\n') {
            let trimmed = line.trim_end_matches('\r').trim();
            if trimmed.is_empty() || seen.iter().any(|item| item == trimmed) {
                continue;
            }
            seen.push(trimmed.to_string());
            lines.push(trimmed.to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn unique_coordinate(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut unique = Vec::new();
    for value in values.filter(|value| value.is_finite()) {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    if unique.len() == 1 {
        Some(unique[0])
    } else {
        None
    }
}

fn members_of<'a>(ids: &'a [String], member: &[&str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    for id in ids {
        if member.contains(&id.as_str()) && !out.contains(&id.as_str()) {
            out.push(id.as_str());
        }
    }
    out
}

fn suggested_keep(assets: &[Value]) -> Vec<String> {
    if assets.is_empty() {
        return Vec::new();
    }
    let mut ordered: Vec<&Value> = assets.iter().collect();
    ordered.sort_by_key(|asset| file_size(asset));
    let largest = file_size(ordered[ordered.len() - 1]);
    ordered.retain(|asset| file_size(asset) == largest);
    if ordered.len() >= 2 {
        ordered.sort_by_key(|asset| exif_count(asset));
    }
    ordered
        .last()
        .and_then(|asset| asset["id"].as_str())
        .map(|id| vec![id.to_string()])
        .unwrap_or_default()
}

fn file_size(asset: &Value) -> i64 {
    asset["exifInfo"]["fileSizeInByte"]
        .as_i64()
        .or_else(|| {
            asset["exifInfo"]["fileSizeInByte"]
                .as_u64()
                .map(|value| value as i64)
        })
        .unwrap_or(0)
}

fn exif_count(asset: &Value) -> usize {
    asset["exifInfo"]
        .as_object()
        .map(|object| object.values().filter(|value| json_truthy(value)).count())
        .unwrap_or(0)
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn parse_groups(body: &Value) -> Result<Vec<ResolveGroup>, DbError> {
    let Some(groups) = body.get("groups").and_then(|value| value.as_array()) else {
        return Err(DbError::Validation);
    };
    if groups.is_empty() {
        return Err(DbError::Validation);
    }
    let mut parsed = Vec::new();
    for group in groups {
        let Some(group) = group.as_object() else {
            return Err(DbError::Validation);
        };
        let Some(duplicate_id) = group.get("duplicateId").and_then(|value| value.as_str()) else {
            return Err(DbError::Validation);
        };
        if !is_uuid_v4(duplicate_id) {
            return Err(DbError::Validation);
        }
        parsed.push(ResolveGroup {
            duplicate_id: duplicate_id.to_string(),
            keep: uuid_list(group.get("keepAssetIds"))?,
            trash: uuid_list(group.get("trashAssetIds"))?,
        });
    }
    Ok(parsed)
}

fn uuid_list(value: Option<&Value>) -> Result<Vec<String>, DbError> {
    let Some(list) = value.and_then(|item| item.as_array()) else {
        return Err(DbError::Validation);
    };
    let mut ids = Vec::new();
    for item in list {
        let Some(id) = item.as_str() else {
            return Err(DbError::Validation);
        };
        if !is_uuid_v4(id) {
            return Err(DbError::Validation);
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn is_uuid_v4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version() == Some(uuid::Version::Random))
}

pub(crate) fn append_locked(existing: Option<&str>, keys: &[&str]) -> String {
    let mut locked = locked_list(existing);
    for key in keys {
        remember(&mut locked, key);
    }
    serde_json::to_string(&locked).unwrap_or_else(|_| "[]".to_string())
}

fn remember(locked: &mut Vec<String>, key: &str) {
    if !locked.iter().any(|item| item == key) {
        locked.push(key.to_string());
    }
}

fn locked_list(text: Option<&str>) -> Vec<String> {
    let Some(text) = text else {
        return Vec::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_str(text) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

fn xmp_document(source: &SidecarSource, locked: &[String]) -> Option<String> {
    let mut body = String::new();
    if locked.iter().any(|key| key == "description") {
        body.push_str(&format!(
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>",
            xml_escape(&source.description)
        ));
    }
    if locked.iter().any(|key| key == "dateTimeOriginal") {
        if let Some(date) = &source.date_time_original {
            let date = xml_escape(date);
            body.push_str(&format!(
                "<photoshop:DateCreated>{date}</photoshop:DateCreated>"
            ));
            body.push_str(&format!(
                "<exif:DateTimeOriginal>{date}</exif:DateTimeOriginal>"
            ));
        }
    }
    if locked.iter().any(|key| key == "timeZone") {
        if let Some(zone) = &source.time_zone {
            body.push_str(&format!(
                "<exif:OffsetTimeOriginal>{}</exif:OffsetTimeOriginal>",
                xml_escape(zone)
            ));
        }
    }
    if locked.iter().any(|key| key == "latitude") {
        if let Some(latitude) = source.latitude {
            body.push_str(&format!("<exif:GPSLatitude>{latitude}</exif:GPSLatitude>"));
        }
    }
    if locked.iter().any(|key| key == "longitude") {
        if let Some(longitude) = source.longitude {
            body.push_str(&format!(
                "<exif:GPSLongitude>{longitude}</exif:GPSLongitude>"
            ));
        }
    }
    if locked.iter().any(|key| key == "rating") {
        let rating = source.rating.unwrap_or(0);
        body.push_str(&format!("<xmp:Rating>{rating}</xmp:Rating>"));
    }
    if locked.iter().any(|key| key == "tags") {
        if let Some(tags) = tag_values(source.tags.as_deref()) {
            body.push_str("<dc:subject><rdf:Bag>");
            for tag in tags {
                body.push_str(&format!("<rdf:li>{}</rdf:li>", xml_escape(&tag)));
            }
            body.push_str("</rdf:Bag></dc:subject>");
        }
    }
    if body.is_empty() {
        return None;
    }
    Some(format!(
        "<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         <rdf:Description rdf:about=\"\"\n\
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
         xmlns:exif=\"http://ns.adobe.com/exif/1.0/\"\n\
         xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
         xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\">{body}</rdf:Description>\n\
         </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
    ))
}

fn tag_values(text: Option<&str>) -> Option<Vec<String>> {
    let text = text?;
    let Value::Array(items) = serde_json::from_str(text).ok()? else {
        return None;
    };
    Some(
        items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn json_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SEARCH_LIMIT;
    use crate::config::Config;
    use crate::db::{new_update_id, App};
    use crate::ml::floats_to_bytes;

    fn config(dir: &std::path::Path) -> Config {
        Config {
            data_dir: dir.to_path_buf(),
            host: std::net::IpAddr::from([127, 0, 0, 1]),
            port: 0,
            admin_email: Some("admin@example.com".to_string()),
            admin_name: Some("Ada".to_string()),
            admin_password: Some("correct horse".to_string()),
            s3: None,
            web_root: dir.join("web"),
            machine_learning_url: "http://immich-machine-learning:3003".to_string(),
            machine_learning_enabled: true,
            config_file: None,
            log_level: None,
        }
    }

    async fn test_app(name: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("immich-dup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = App::prepare(&config(&dir)).await.expect("app");
        (app, dir)
    }

    async fn owner_id(app: &App) -> String {
        sqlx::query_scalar("SELECT id FROM user LIMIT 1")
            .fetch_one(app.pool())
            .await
            .unwrap()
    }

    async fn insert_asset(
        app: &App,
        owner: &str,
        id: &str,
        kind: &str,
        visibility: &str,
        stack: Option<&str>,
        deleted: bool,
    ) {
        sqlx::query(
            "INSERT INTO asset (
                id, ownerId, type, originalPath, originalFileName, originalMimeType,
                fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
                stackId, deletedAt, status, createdAt, updatedAt, updateId
             ) VALUES (
                ?, ?, ?, 'object', 'a.jpg', 'image/jpeg',
                '2020-01-02T03:04:05.000Z', '2020-01-02T03:04:05.000Z', '2020-01-02T03:04:05.000Z',
                ?, ?, ?, ?, 'active',
                '2020-01-02T03:04:05.000Z', '2020-01-02T03:04:05.000Z', ?
             )",
        )
        .bind(id)
        .bind(owner)
        .bind(kind)
        .bind(id.as_bytes())
        .bind(visibility)
        .bind(stack)
        .bind(deleted.then_some("2020-02-01T00:00:00.000Z"))
        .bind(new_update_id())
        .execute(app.pool())
        .await
        .unwrap();
    }

    async fn insert_clip(app: &App, id: &str, values: &[f32]) {
        sqlx::query("INSERT INTO asset_clip (assetId, embedding) VALUES (?, ?)")
            .bind(id)
            .bind(floats_to_bytes(values))
            .execute(app.pool())
            .await
            .unwrap();
    }

    async fn duplicate_id(app: &App, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT duplicateId FROM asset WHERE id = ?")
            .bind(id)
            .fetch_one(app.pool())
            .await
            .unwrap()
    }

    async fn detected_at(app: &App, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT duplicatesDetectedAt FROM asset_job_status WHERE assetId = ?")
            .bind(id)
            .fetch_optional(app.pool())
            .await
            .unwrap()
            .flatten()
    }

    fn asset_uuid(n: u32) -> String {
        format!("00000000-0000-4000-8000-{n:012}")
    }

    #[tokio::test]
    async fn exact_cosine_groups_only_the_nearest_visible_assets() {
        let (app, dir) = test_app("cosine").await;
        let owner = owner_id(&app).await;
        let source = asset_uuid(1);
        let close = asset_uuid(2);
        let far = asset_uuid(3);
        let stacked = asset_uuid(4);
        let hidden = asset_uuid(5);
        let locked = asset_uuid(6);
        let deleted = asset_uuid(7);
        let video = asset_uuid(8);
        let missing = asset_uuid(9);
        for (id, kind, visibility, stack, gone) in [
            (source.as_str(), "IMAGE", "timeline", None, false),
            (close.as_str(), "IMAGE", "archive", None, false),
            (far.as_str(), "IMAGE", "timeline", None, false),
            (
                stacked.as_str(),
                "IMAGE",
                "timeline",
                Some(asset_uuid(50).as_str()),
                false,
            ),
            (hidden.as_str(), "IMAGE", "hidden", None, false),
            (locked.as_str(), "IMAGE", "locked", None, false),
            (deleted.as_str(), "IMAGE", "timeline", None, true),
            (video.as_str(), "VIDEO", "timeline", None, false),
            (missing.as_str(), "IMAGE", "timeline", None, false),
        ] {
            insert_asset(&app, &owner, id, kind, visibility, stack, gone).await;
        }
        insert_clip(&app, &source, &[1.0, 0.0]).await;
        insert_clip(&app, &close, &[1.0, 0.01]).await;
        insert_clip(&app, &far, &[1.0, 1.0]).await;
        insert_clip(&app, &stacked, &[1.0, 0.0]).await;
        insert_clip(&app, &hidden, &[1.0, 0.0]).await;
        insert_clip(&app, &locked, &[1.0, 0.0]).await;
        insert_clip(&app, &deleted, &[1.0, 0.0]).await;
        insert_clip(&app, &video, &[1.0, 0.0]).await;

        app.detect_duplicates(&missing, false).await.unwrap();
        assert_eq!(duplicate_id(&app, &missing).await, None);
        assert_eq!(detected_at(&app, &missing).await, None);

        app.detect_duplicates(&source, false).await.unwrap();
        let group = duplicate_id(&app, &source).await.expect("group");
        assert_eq!(
            duplicate_id(&app, &close).await.as_deref(),
            Some(group.as_str())
        );
        assert_eq!(duplicate_id(&app, &far).await, None);
        assert_eq!(duplicate_id(&app, &stacked).await, None);
        assert_eq!(duplicate_id(&app, &hidden).await, None);
        assert_eq!(duplicate_id(&app, &locked).await, None);
        assert_eq!(duplicate_id(&app, &deleted).await, None);
        assert_eq!(duplicate_id(&app, &video).await, None);
        assert!(detected_at(&app, &source).await.is_some());

        sqlx::query(
            "INSERT INTO system_metadata (key, value) VALUES ('system-config', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(r#"{"machineLearning":{"duplicateDetection":{"maxDistance":0.5}}}"#)
        .execute(app.pool())
        .await
        .unwrap();
        app.detect_duplicates(&source, false).await.unwrap();
        assert_eq!(duplicate_id(&app, &far).await, None);
        app.detect_duplicates(&source, true).await.unwrap();
        assert_eq!(
            duplicate_id(&app, &far).await.as_deref(),
            Some(group.as_str())
        );

        sqlx::query("DELETE FROM system_metadata WHERE key = 'system-config'")
            .execute(app.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE asset SET duplicateId = ? WHERE id = ?")
            .bind(&group)
            .bind(&far)
            .execute(app.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE asset_clip SET embedding = ? WHERE assetId = ?")
            .bind(floats_to_bytes(&[0.0, 1.0]))
            .bind(&source)
            .execute(app.pool())
            .await
            .unwrap();
        app.detect_duplicates(&source, true).await.unwrap();
        assert_eq!(duplicate_id(&app, &source).await, None);
        assert_eq!(
            duplicate_id(&app, &far).await.as_deref(),
            Some(group.as_str())
        );

        app.detect_duplicates(&hidden, false).await.unwrap();
        assert_eq!(detected_at(&app, &hidden).await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detection_keeps_the_nearest_sixty_four() {
        let (app, dir) = test_app("limit").await;
        let owner = owner_id(&app).await;
        let source = asset_uuid(100);
        insert_asset(&app, &owner, &source, "IMAGE", "timeline", None, false).await;
        insert_clip(&app, &source, &[1.0, 0.0]).await;
        for n in 1..=70 {
            let id = asset_uuid(n);
            insert_asset(&app, &owner, &id, "IMAGE", "timeline", None, false).await;
            insert_clip(&app, &id, &[1.0, 0.0]).await;
        }
        app.detect_duplicates(&source, false).await.unwrap();
        let group = duplicate_id(&app, &source).await.expect("group");
        for n in 1..=SEARCH_LIMIT as u32 {
            assert_eq!(
                duplicate_id(&app, &asset_uuid(n)).await.as_deref(),
                Some(group.as_str()),
                "asset {n}"
            );
        }
        for n in (SEARCH_LIMIT as u32 + 1)..=70 {
            assert_eq!(duplicate_id(&app, &asset_uuid(n)).await, None, "asset {n}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_resolve_and_trash_follow_the_group() {
        let (app, dir) = test_app("resolve").await;
        let owner = owner_id(&app).await;
        let first = asset_uuid(1);
        let second = asset_uuid(2);
        let third = asset_uuid(3);
        for id in [&first, &second, &third] {
            insert_asset(&app, &owner, id, "IMAGE", "timeline", None, false).await;
            insert_clip(&app, id, &[1.0, 0.0]).await;
            sqlx::query(
                "INSERT INTO asset_exif (assetId, description, fileSizeInByte, updatedAt, updateId)
                 VALUES (?, '', ?, '2020-01-02T03:04:05.000Z', ?)",
            )
            .bind(id)
            .bind(if id == &second { 50 } else { 10 })
            .bind(new_update_id())
            .execute(app.pool())
            .await
            .unwrap();
        }
        sqlx::query("UPDATE asset_exif SET make = 'Canon' WHERE assetId = ?")
            .bind(&first)
            .execute(app.pool())
            .await
            .unwrap();
        app.detect_duplicates(&first, false).await.unwrap();
        let group = duplicate_id(&app, &first).await.expect("group");
        let listed = app.list_duplicates(&owner).await.unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(
            listed[0]["suggestedKeepAssetIds"][0].as_str(),
            Some(second.as_str())
        );

        sqlx::query("UPDATE asset SET visibility = 'hidden' WHERE duplicateId = ?")
            .bind(&group)
            .execute(app.pool())
            .await
            .unwrap();
        let hidden = app
            .resolve_duplicates(
                &owner,
                &serde_json::json!({
                    "groups": [{
                        "duplicateId": group,
                        "keepAssetIds": [first],
                        "trashAssetIds": [],
                    }]
                }),
            )
            .await
            .unwrap();
        assert_eq!(hidden[0]["success"], false);
        assert_eq!(hidden[0]["error"], "not_found");

        sqlx::query("UPDATE asset SET visibility = 'timeline', duplicateId = ?")
            .bind(&group)
            .execute(app.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE asset SET deletedAt = '2020-03-01T00:00:00.000Z' WHERE id = ?")
            .bind(&third)
            .execute(app.pool())
            .await
            .unwrap();
        let collapsed = app.list_duplicates(&owner).await.unwrap();
        assert_eq!(collapsed.as_array().unwrap().len(), 1);
        sqlx::query("UPDATE asset SET deletedAt = '2020-03-01T00:00:00.000Z' WHERE id = ?")
            .bind(&second)
            .execute(app.pool())
            .await
            .unwrap();
        let empty = app.list_duplicates(&owner).await.unwrap();
        assert_eq!(empty.as_array().unwrap().len(), 0);
        assert_eq!(duplicate_id(&app, &first).await, None);

        sqlx::query("UPDATE asset SET deletedAt = NULL, duplicateId = ? WHERE ownerId = ?")
            .bind(&group)
            .bind(&owner)
            .execute(app.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO system_metadata (key, value) VALUES ('system-config', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(r#"{"trash":{"enabled":false}}"#)
        .execute(app.pool())
        .await
        .unwrap();
        let resolved = app
            .resolve_duplicates(
                &owner,
                &serde_json::json!({
                    "groups": [{
                        "duplicateId": group,
                        "keepAssetIds": [first],
                        "trashAssetIds": [second, third],
                    }]
                }),
            )
            .await
            .unwrap();
        assert_eq!(resolved[0]["success"], true);
        assert_eq!(duplicate_id(&app, &first).await, None);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset WHERE id IN (?, ?)")
            .bind(&second)
            .bind(&third)
            .fetch_one(app.pool())
            .await
            .unwrap();
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
