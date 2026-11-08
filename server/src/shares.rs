//! Shared links, the album timeline a recipient opens, and the download zip.
//! The zip contains Originals only. A sidecar is not an entry.

use std::collections::HashMap;

use axum::http::StatusCode;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};

const FOUR_GIB: i64 = 4 * 1024 * 1024 * 1024;
const LINK_COLUMNS: &str = "id, userId AS user_id, key, type AS link_type, createdAt AS created_at,
    expiresAt AS expires_at, allowUpload AS allow_upload, albumId AS album_id,
    allowDownload AS allow_download, showExif AS show_exif, password, slug, description";

pub(crate) enum ApiError {
    Db(DbError),
    Message(StatusCode, String),
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<crate::store::StoreError> for ApiError {
    fn from(error: crate::store::StoreError) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl ApiError {
    pub(crate) fn text(status: StatusCode, text: impl Into<String>) -> Self {
        Self::Message(status, text.into())
    }
}

pub(crate) struct Viewer {
    pub user_id: String,
    pub share: ShareGrant,
}

pub(crate) struct ShareGrant {
    pub id: String,
    pub allow_download: bool,
    pub allow_upload: bool,
    pub show_metadata: bool,
    pub album_id: Option<String>,
    pub link_type: String,
    pub password: Option<String>,
}

pub(crate) enum ShareLookup {
    Invalid,
    Found(Viewer),
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: String,
    user_id: String,
    key: Vec<u8>,
    link_type: String,
    created_at: String,
    expires_at: Option<String>,
    allow_upload: i64,
    album_id: Option<String>,
    allow_download: i64,
    show_exif: i64,
    password: Option<String>,
    slug: Option<String>,
    description: Option<String>,
}

#[derive(sqlx::FromRow)]
struct BucketAsset {
    id: String,
    duration: Option<i64>,
    visibility: String,
    is_favorite: i64,
    asset_type: String,
    live_photo_video_id: Option<String>,
    file_created_at: String,
    local_date_time: String,
    created_at: String,
    owner_id: String,
    width: Option<i64>,
    height: Option<i64>,
    thumbhash: Option<Vec<u8>>,
    projection_type: Option<String>,
    city: Option<String>,
    country: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct DownloadRow {
    id: String,
    size: Option<i64>,
    live_photo_video_id: Option<String>,
    original_path: String,
    original_file_name: String,
}

#[derive(sqlx::FromRow)]
struct MarkerRow {
    id: String,
    lat: f64,
    lon: f64,
    city: Option<String>,
    state: Option<String>,
    country: Option<String>,
}

struct Archive {
    size: i64,
    asset_ids: Vec<String>,
}

pub(crate) enum DocumentLink {
    Missing,
    Quiet,
    Tagged {
        title: String,
        description: String,
        image_url: String,
    },
}

impl App {
    pub(crate) async fn lookup_share(
        &self,
        by_key: bool,
        token: &str,
    ) -> Result<ShareLookup, DbError> {
        let row = if by_key {
            let Some(bytes) = decode_key(token) else {
                return Ok(ShareLookup::Invalid);
            };
            self.link_where("key = ?", &bytes).await?
        } else {
            self.link_where_text("slug = ?", token).await?
        };
        let Some(row) = row else {
            return Ok(ShareLookup::Invalid);
        };
        if !self.link_is_open(&row).await? {
            return Ok(ShareLookup::Invalid);
        }
        Ok(ShareLookup::Found(viewer_from(&row)))
    }

    pub(crate) async fn list_links(
        &self,
        user_id: &str,
        album_id: Option<&str>,
        id: Option<&str>,
    ) -> Result<Value, ApiError> {
        let mut sql = format!("SELECT {LINK_COLUMNS} FROM shared_link WHERE userId = ?");
        if album_id.is_some() {
            sql.push_str(" AND albumId = ?");
        }
        if id.is_some() {
            sql.push_str(" AND id = ?");
        }
        sql.push_str(" ORDER BY createdAt DESC");
        let mut query = sqlx::query_as::<_, LinkRow>(&sql).bind(user_id);
        if let Some(album_id) = album_id {
            query = query.bind(album_id);
        }
        if let Some(id) = id {
            query = query.bind(id);
        }
        let rows = query.fetch_all(self.pool()).await?;
        let mut links = Vec::new();
        for row in rows {
            if self.link_is_open(&row).await? {
                links.push(self.link_json(&row, false, false).await?);
            }
        }
        Ok(Value::Array(links))
    }

    pub(crate) async fn create_link(&self, user_id: &str, body: &Value) -> Result<Value, ApiError> {
        let link_type = body.get("type").and_then(Value::as_str).unwrap_or("");
        let asset_ids = string_list(body.get("assetIds"));
        let album_id = body.get("albumId").and_then(Value::as_str);
        match link_type {
            "ALBUM" => {
                if !asset_ids.is_empty() || album_id.map(str::is_empty).unwrap_or(true) {
                    return Err(ApiError::text(StatusCode::BAD_REQUEST, "Validation failed"));
                }
                self.require_album_share(user_id, album_id.unwrap()).await?;
            }
            "INDIVIDUAL" => {
                if album_id.is_some() || asset_ids.is_empty() {
                    return Err(ApiError::text(StatusCode::BAD_REQUEST, "Validation failed"));
                }
                self.require_assets(user_id, &asset_ids, "asset.share")
                    .await?;
            }
            _ => return Err(ApiError::text(StatusCode::BAD_REQUEST, "Validation failed")),
        }
        let show_metadata = body
            .get("showMetadata")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let allow_download = if show_metadata {
            body.get("allowDownload")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        } else {
            false
        };
        let allow_upload = body
            .get("allowUpload")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let mut key = [0_u8; 50];
        getrandom::fill(&mut key).map_err(|error| DbError::Store(error.to_string()))?;
        let id = new_id();
        let inserted = sqlx::query(
            "INSERT INTO shared_link
                (id, description, userId, key, type, createdAt, expiresAt, allowUpload, albumId,
                 allowDownload, showExif, password, slug)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(blank_to_null(body.get("description")))
        .bind(user_id)
        .bind(key.as_slice())
        .bind(link_type)
        .bind(now_iso())
        .bind(blank_to_null(body.get("expiresAt")))
        .bind(i64::from(allow_upload))
        .bind(if link_type == "ALBUM" { album_id } else { None })
        .bind(i64::from(allow_download))
        .bind(i64::from(show_metadata))
        .bind(blank_to_null(body.get("password")))
        .bind(blank_to_null(body.get("slug")))
        .execute(self.pool())
        .await;
        if let Err(error) = inserted {
            if slug_taken(&error) {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Failed to save shared link",
                ));
            }
            return Err(error.into());
        }
        for asset_id in &asset_ids {
            sqlx::query("INSERT INTO shared_link_asset (assetId, sharedLinkId) VALUES (?, ?)")
                .bind(asset_id)
                .bind(&id)
                .execute(self.pool())
                .await?;
        }
        let row = self
            .link_where_text("id = ?", &id)
            .await?
            .ok_or_else(|| ApiError::text(StatusCode::BAD_REQUEST, "Shared link not found"))?;
        self.link_json(&row, false, false).await
    }

    pub(crate) async fn update_link(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, ApiError> {
        let _existing = self.owned_link(user_id, id).await?;
        let slug = blank_to_null(body.get("slug"));
        let mut sql = String::from("UPDATE shared_link SET slug = ?");
        if body.get("description").is_some() {
            sql.push_str(", description = ?");
        }
        if body.get("password").is_some() {
            sql.push_str(", password = ?");
        }
        if body.get("expiresAt").is_some() {
            sql.push_str(", expiresAt = ?");
        }
        if body.get("allowUpload").and_then(Value::as_bool).is_some() {
            sql.push_str(", allowUpload = ?");
        }
        if body.get("allowDownload").and_then(Value::as_bool).is_some() {
            sql.push_str(", allowDownload = ?");
        }
        if body.get("showMetadata").and_then(Value::as_bool).is_some() {
            sql.push_str(", showExif = ?");
        }
        sql.push_str(" WHERE id = ? AND userId = ?");
        let mut query = sqlx::query(&sql).bind(slug);
        if body.get("description").is_some() {
            query = query.bind(optional_text(body.get("description")));
        }
        if body.get("password").is_some() {
            query = query.bind(optional_text(body.get("password")));
        }
        if body.get("expiresAt").is_some() {
            query = query.bind(optional_text(body.get("expiresAt")));
        }
        if let Some(value) = body.get("allowUpload").and_then(Value::as_bool) {
            query = query.bind(i64::from(value));
        }
        if let Some(value) = body.get("allowDownload").and_then(Value::as_bool) {
            query = query.bind(i64::from(value));
        }
        if let Some(value) = body.get("showMetadata").and_then(Value::as_bool) {
            query = query.bind(i64::from(value));
        }
        let updated = query.bind(id).bind(user_id).execute(self.pool()).await;
        if let Err(error) = updated {
            if slug_taken(&error) {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Failed to save shared link",
                ));
            }
            return Err(error.into());
        }
        let row = self.owned_link(user_id, id).await?;
        self.link_json(&row, false, false).await
    }

    pub(crate) async fn delete_link(&self, user_id: &str, id: &str) -> Result<(), ApiError> {
        let row = self.owned_link(user_id, id).await?;
        sqlx::query("DELETE FROM shared_link WHERE id = ?")
            .bind(&row.id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn link_by_id(&self, user_id: &str, id: &str) -> Result<Value, ApiError> {
        let row = self.owned_link(user_id, id).await?;
        self.link_json(&row, false, false).await
    }

    pub(crate) async fn public_link(
        &self,
        viewer: &Viewer,
        strip: bool,
    ) -> Result<Value, ApiError> {
        let row = self
            .link_where_text("id = ?", &viewer.share.id)
            .await?
            .filter(|row| row.user_id == viewer.user_id)
            .ok_or_else(|| ApiError::text(StatusCode::BAD_REQUEST, "Shared link not found"))?;
        self.link_json(&row, strip, true).await
    }

    pub(crate) fn password_token(id: &str, password: &str) -> String {
        let digest = Sha256::digest(format!("{id}-{password}").as_bytes());
        base64::engine::general_purpose::STANDARD.encode(digest)
    }

    pub(crate) async fn add_link_assets(
        &self,
        user_id: &str,
        id: &str,
        asset_ids: &[String],
    ) -> Result<Value, ApiError> {
        let row = self.owned_link(user_id, id).await?;
        if row.link_type != "INDIVIDUAL" {
            return Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Invalid shared link type",
            ));
        }
        let existing = self.link_asset_ids(&row.id).await?;
        let mut results = Vec::new();
        let mut added = Vec::new();
        for asset_id in asset_ids {
            if existing.iter().any(|id| id == asset_id) {
                results
                    .push(json!({ "assetId": asset_id, "success": false, "error": "duplicate" }));
                continue;
            }
            let owned: Option<String> =
                sqlx::query_scalar("SELECT id FROM asset WHERE id = ? AND ownerId = ?")
                    .bind(asset_id)
                    .bind(user_id)
                    .fetch_optional(self.pool())
                    .await?;
            if owned.is_none() {
                results.push(json!({
                    "assetId": asset_id,
                    "success": false,
                    "error": "no_permission"
                }));
                continue;
            }
            added.push(asset_id.clone());
            results.push(json!({ "assetId": asset_id, "success": true }));
        }
        for asset_id in added {
            sqlx::query("INSERT INTO shared_link_asset (assetId, sharedLinkId) VALUES (?, ?)")
                .bind(asset_id)
                .bind(&row.id)
                .execute(self.pool())
                .await?;
        }
        Ok(Value::Array(results))
    }

    pub(crate) async fn remove_link_assets(
        &self,
        user_id: &str,
        id: &str,
        asset_ids: &[String],
    ) -> Result<Value, ApiError> {
        let row = self.owned_link(user_id, id).await?;
        if row.link_type != "INDIVIDUAL" {
            return Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Invalid shared link type",
            ));
        }
        let mut results = Vec::new();
        for asset_id in asset_ids {
            let removed =
                sqlx::query("DELETE FROM shared_link_asset WHERE sharedLinkId = ? AND assetId = ?")
                    .bind(&row.id)
                    .bind(asset_id)
                    .execute(self.pool())
                    .await?;
            if removed.rows_affected() == 0 {
                results
                    .push(json!({ "assetId": asset_id, "success": false, "error": "not_found" }));
            } else {
                results.push(json!({ "assetId": asset_id, "success": true }));
            }
        }
        Ok(Value::Array(results))
    }

    pub(crate) async fn attach_shared_upload(
        &self,
        grant: &ShareGrant,
        asset_id: &str,
    ) -> Result<(), DbError> {
        if grant.link_type == "ALBUM" {
            let Some(album_id) = &grant.album_id else {
                return Ok(());
            };
            let inserted = sqlx::query(
                "INSERT INTO album_asset (albumId, assetId, createdAt, updateId) VALUES (?, ?, ?, ?)",
            )
            .bind(album_id)
            .bind(asset_id)
            .bind(now_iso())
            .bind(new_update_id())
            .execute(self.pool())
            .await;
            if let Err(error) = inserted {
                if !unique_constraint(&error) {
                    return Err(error.into());
                }
                return Ok(());
            }
            sqlx::query(
                "UPDATE album SET albumThumbnailAssetId = COALESCE(albumThumbnailAssetId, ?), updatedAt = ?, updateId = ? WHERE id = ?",
            )
            .bind(asset_id)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(album_id)
            .execute(self.pool())
            .await?;
            return Ok(());
        }
        let inserted =
            sqlx::query("INSERT INTO shared_link_asset (assetId, sharedLinkId) VALUES (?, ?)")
                .bind(asset_id)
                .bind(&grant.id)
                .execute(self.pool())
                .await;
        if let Err(error) = inserted {
            if unique_constraint(&error) {
                return Ok(());
            }
            return Err(error.into());
        }
        Ok(())
    }

    pub(crate) async fn share_can_see(
        &self,
        user_id: &str,
        grant: &ShareGrant,
        asset_id: &str,
        download: bool,
    ) -> Result<bool, DbError> {
        if download && !grant.allow_download {
            return Ok(false);
        }
        let found: Option<String> = sqlx::query_scalar(
            "SELECT asset.id FROM asset
             WHERE asset.id = ? AND asset.ownerId = ? AND asset.deletedAt IS NULL
             AND (
                EXISTS (
                    SELECT 1 FROM shared_link_asset
                    WHERE sharedLinkId = ? AND assetId = asset.id
                )
                OR EXISTS (
                    SELECT 1 FROM album_asset
                    WHERE albumId = ? AND assetId = asset.id
                )
             )",
        )
        .bind(asset_id)
        .bind(user_id)
        .bind(&grant.id)
        .bind(grant.album_id.as_deref().unwrap_or(""))
        .fetch_optional(self.pool())
        .await?;
        Ok(found.is_some())
    }

    pub(crate) async fn timeline_buckets(
        &self,
        request: &TimelineRequest<'_>,
    ) -> Result<Value, ApiError> {
        self.require_timeline_album(request.user_id, request.grant, request.album_id)
            .await?;
        let partners = timeline_partners(request);
        let desc = request.order != "asc";
        let sql = bucket_month_sql(request.album_id.is_some(), partners, desc);
        let mut query = sqlx::query_as::<_, (String, i64)>(&sql);
        query = match request.album_id {
            Some(album_id) => query.bind(album_id),
            None => query.bind(request.user_id),
        };
        if partners {
            query = query.bind(request.user_id);
        }
        let rows = query.fetch_all(self.pool()).await?;
        let buckets = rows
            .into_iter()
            .map(|(month, count)| json!({ "timeBucket": format!("{month}-01"), "count": count }))
            .collect();
        Ok(Value::Array(buckets))
    }

    pub(crate) async fn timeline_bucket(
        &self,
        request: &TimelineRequest<'_>,
        time_bucket: &str,
        with_coordinates: bool,
    ) -> Result<Value, ApiError> {
        self.require_timeline_album(request.user_id, request.grant, request.album_id)
            .await?;
        let show_place = request.grant.is_none_or(|grant| grant.show_metadata);
        let coordinates = with_coordinates && show_place;
        let partners = timeline_partners(request);
        let month = month_key(time_bucket);
        let desc = request.order != "asc";
        let sql = bucket_asset_sql(request.album_id.is_some(), partners, desc);
        let mut query = sqlx::query_as::<_, BucketAsset>(&sql).bind(month);
        query = match request.album_id {
            Some(album_id) => query.bind(album_id),
            None => query.bind(request.user_id),
        };
        if partners {
            query = query.bind(request.user_id);
        }
        let rows = query.fetch_all(self.pool()).await?;
        Ok(bucket_json(&rows, show_place, coordinates))
    }

    pub(crate) async fn map_markers(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        album_id: &str,
    ) -> Result<Value, ApiError> {
        if let Some(grant) = grant {
            if grant.album_id.as_deref() != Some(album_id) {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Not found or no album.read access",
                ));
            }
            if !grant.show_metadata {
                return Ok(Value::Array(Vec::new()));
            }
        } else if !self.can_read_album(user_id, album_id).await? {
            return Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Not found or no album.read access",
            ));
        }
        let rows = sqlx::query_as::<_, MarkerRow>(
            "SELECT asset.id AS id, asset_exif.latitude AS lat, asset_exif.longitude AS lon,
                    asset_exif.city AS city, asset_exif.state AS state, asset_exif.country AS country
             FROM album_asset
             JOIN asset ON asset.id = album_asset.assetId
             JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE album_asset.albumId = ? AND asset.deletedAt IS NULL
               AND asset_exif.latitude IS NOT NULL AND asset_exif.longitude IS NOT NULL",
        )
        .bind(album_id)
        .fetch_all(self.pool())
        .await?;
        let markers = rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "lat": row.lat,
                    "lon": row.lon,
                    "city": row.city,
                    "state": row.state,
                    "country": row.country,
                })
            })
            .collect();
        Ok(Value::Array(markers))
    }

    pub(crate) async fn download_info(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        body: &Value,
    ) -> Result<Value, ApiError> {
        let assets = self.download_assets(user_id, grant, body).await?;
        let limit = body
            .get("archiveSize")
            .and_then(Value::as_i64)
            .filter(|size| *size > 0)
            .unwrap_or(FOUR_GIB);
        let mut motion_ids = Vec::new();
        let mut archives = Vec::new();
        let mut current = Archive {
            size: 0,
            asset_ids: Vec::new(),
        };
        for asset in &assets {
            if let Some(motion) = &asset.live_photo_video_id {
                motion_ids.push(motion.clone());
            }
            push_archive(
                &mut archives,
                &mut current,
                &asset.id,
                asset.size.unwrap_or(0),
                limit,
            );
        }
        for motion_id in motion_ids {
            if assets.iter().any(|asset| asset.id == motion_id) {
                continue;
            }
            if let Some(motion) = self.download_row(&motion_id).await? {
                if motion.original_path.contains("encoded-video") {
                    continue;
                }
                push_archive(
                    &mut archives,
                    &mut current,
                    &motion.id,
                    motion.size.unwrap_or(0),
                    limit,
                );
            }
        }
        if !current.asset_ids.is_empty() {
            archives.push(current);
        }
        let total_size: i64 = archives.iter().map(|archive| archive.size).sum();
        let archives: Vec<Value> = archives
            .into_iter()
            .map(|archive| json!({ "size": archive.size, "assetIds": archive.asset_ids }))
            .collect();
        Ok(json!({ "totalSize": total_size, "archives": archives }))
    }

    pub(crate) async fn download_zip(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        asset_ids: &[String],
    ) -> Result<Vec<u8>, ApiError> {
        if asset_ids.is_empty() {
            return Ok(write_zip(&[]));
        }
        self.require_assets_download(user_id, grant, asset_ids)
            .await?;
        let mut names = HashMap::new();
        let mut entries = Vec::new();
        for asset_id in asset_ids {
            let Some(row) = self.download_row(asset_id).await? else {
                continue;
            };
            let Some(bytes) = self.store().read_file(&row.original_path).await? else {
                continue;
            };
            let filename = unique_name(&mut names, &sanitize_name(&row.original_file_name));
            entries.push((filename, bytes));
        }
        Ok(write_zip(&entries))
    }

    pub(crate) async fn document_link(
        &self,
        by_key: bool,
        token: &str,
        origin: &str,
    ) -> Result<DocumentLink, DbError> {
        let ShareLookup::Found(viewer) = self.lookup_share(by_key, token).await? else {
            return Ok(DocumentLink::Missing);
        };
        if viewer.share.password.is_some() {
            return Ok(DocumentLink::Quiet);
        }
        let row = self
            .link_where_text("id = ?", &viewer.share.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let value = self
            .link_json(&row, false, false)
            .await
            .map_err(|error| match error {
                ApiError::Db(error) => error,
                ApiError::Message(_, _) => DbError::NotFound,
            })?;
        let title = value
            .get("album")
            .and_then(|album| album.get("albumName"))
            .and_then(Value::as_str)
            .unwrap_or("Public Share")
            .to_string();
        let asset_count = value
            .get("assets")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let album_count = value
            .get("album")
            .and_then(|album| album.get("assetCount"))
            .and_then(Value::as_i64)
            .unwrap_or(0) as usize;
        let count = if asset_count > 0 {
            asset_count
        } else {
            album_count
        };
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{count} shared photos & videos"));
        let asset_id = value
            .pointer("/album/albumThumbnailAssetId")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/assets/0/id").and_then(Value::as_str));
        let key = value
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let image_path = match asset_id {
            Some(asset_id) => format!("/api/assets/{asset_id}/thumbnail?key={key}"),
            None => "/feature-panel.png".to_string(),
        };
        let image_url = format!("{origin}{image_path}");
        Ok(DocumentLink::Tagged {
            title,
            description,
            image_url,
        })
    }

    async fn download_assets(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        body: &Value,
    ) -> Result<Vec<DownloadRow>, ApiError> {
        if let Some(ids) = body.get("assetIds") {
            let ids = string_list(Some(ids));
            self.require_assets_download(user_id, grant, &ids).await?;
            let mut rows = Vec::new();
            for id in ids {
                if let Some(row) = self.download_row(&id).await? {
                    rows.push(row);
                }
            }
            return Ok(rows);
        }
        if let Some(album_id) = body.get("albumId").and_then(Value::as_str) {
            self.require_album_download(user_id, grant, album_id)
                .await?;
            let rows = sqlx::query_as::<_, DownloadRow>(
                "SELECT asset.id AS id, asset_exif.fileSizeInByte AS size,
                        asset.livePhotoVideoId AS live_photo_video_id,
                        asset.originalPath AS original_path,
                        asset.originalFileName AS original_file_name
                 FROM album_asset
                 JOIN asset ON asset.id = album_asset.assetId
                 LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
                 WHERE album_asset.albumId = ? AND asset.deletedAt IS NULL
                 ORDER BY asset.fileCreatedAt ASC",
            )
            .bind(album_id)
            .fetch_all(self.pool())
            .await?;
            return Ok(rows);
        }
        if let Some(target) = body.get("userId").and_then(Value::as_str) {
            if grant.is_some() || target != user_id {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Not found or no timeline.download access",
                ));
            }
            let rows = sqlx::query_as::<_, DownloadRow>(
                "SELECT asset.id AS id, asset_exif.fileSizeInByte AS size,
                        asset.livePhotoVideoId AS live_photo_video_id,
                        asset.originalPath AS original_path,
                        asset.originalFileName AS original_file_name
                 FROM asset
                 LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
                 WHERE asset.ownerId = ? AND asset.deletedAt IS NULL AND asset.visibility = 'timeline'
                 ORDER BY asset.fileCreatedAt ASC",
            )
            .bind(user_id)
            .fetch_all(self.pool())
            .await?;
            return Ok(rows);
        }
        Err(ApiError::text(
            StatusCode::BAD_REQUEST,
            "assetIds, albumId, or userId is required",
        ))
    }

    async fn require_assets_download(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        ids: &[String],
    ) -> Result<(), ApiError> {
        if let Some(grant) = grant {
            for id in ids {
                if !self.share_can_see(user_id, grant, id, true).await? {
                    return Err(ApiError::text(
                        StatusCode::BAD_REQUEST,
                        "Not found or no asset.download access",
                    ));
                }
            }
            return Ok(());
        }
        self.require_assets(user_id, ids, "asset.download").await
    }

    async fn require_album_download(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        album_id: &str,
    ) -> Result<(), ApiError> {
        let allowed = if let Some(grant) = grant {
            grant.allow_download && grant.album_id.as_deref() == Some(album_id)
        } else {
            self.can_read_album(user_id, album_id).await?
        };
        if allowed {
            Ok(())
        } else {
            Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Not found or no album.download access",
            ))
        }
    }

    async fn require_timeline_album(
        &self,
        user_id: &str,
        grant: Option<&ShareGrant>,
        album_id: Option<&str>,
    ) -> Result<(), ApiError> {
        if let Some(grant) = grant {
            let Some(album_id) = album_id else {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Not found or no timeline.read access",
                ));
            };
            if grant.album_id.as_deref() != Some(album_id) {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Not found or no album.read access",
                ));
            }
            return Ok(());
        }
        if let Some(album_id) = album_id {
            if !self.can_read_album(user_id, album_id).await? {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    "Not found or no album.read access",
                ));
            }
        }
        Ok(())
    }

    async fn link_json(&self, row: &LinkRow, strip: bool, public: bool) -> Result<Value, ApiError> {
        let ids = self.link_asset_ids(&row.id).await?;
        let mut assets = Vec::new();
        for id in ids {
            let asset = if public {
                self.share_asset_json(&row.user_id, &id, strip).await?
            } else {
                self.asset_json(&row.user_id, &id).await?
            };
            assets.push(asset);
        }
        let mut value = json!({
            "id": row.id,
            "description": row.description,
            "password": row.password,
            "userId": row.user_id,
            "key": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&row.key),
            "type": row.link_type,
            "createdAt": row.created_at,
            "expiresAt": row.expires_at,
            "assets": assets,
            "allowUpload": row.allow_upload != 0,
            "allowDownload": row.allow_download != 0,
            "showMetadata": row.show_exif != 0,
            "slug": row.slug,
        });
        if row.link_type == "ALBUM" {
            if let Some(album_id) = &row.album_id {
                if let Ok(mut album) = self.album_json(&row.user_id, album_id).await {
                    album["hasSharedLink"] = json!(true);
                    album["shared"] = json!(true);
                    value["album"] = album;
                }
            }
        }
        Ok(value)
    }

    async fn owned_link(&self, user_id: &str, id: &str) -> Result<LinkRow, ApiError> {
        let row = sqlx::query_as::<_, LinkRow>(&format!(
            "SELECT {LINK_COLUMNS} FROM shared_link WHERE id = ? AND userId = ?"
        ))
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else {
            return Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Shared link not found",
            ));
        };
        if !self.link_is_open(&row).await? {
            return Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Shared link not found",
            ));
        }
        Ok(row)
    }

    async fn link_where(&self, clause: &str, key: &[u8]) -> Result<Option<LinkRow>, DbError> {
        let sql = format!("SELECT {LINK_COLUMNS} FROM shared_link WHERE {clause}");
        Ok(sqlx::query_as::<_, LinkRow>(&sql)
            .bind(key)
            .fetch_optional(self.pool())
            .await?)
    }

    async fn link_where_text(&self, clause: &str, value: &str) -> Result<Option<LinkRow>, DbError> {
        let sql = format!("SELECT {LINK_COLUMNS} FROM shared_link WHERE {clause}");
        Ok(sqlx::query_as::<_, LinkRow>(&sql)
            .bind(value)
            .fetch_optional(self.pool())
            .await?)
    }

    async fn link_is_open(&self, row: &LinkRow) -> Result<bool, DbError> {
        if row.expires_at.as_deref().is_some_and(is_expired) {
            return Ok(false);
        }
        if row.link_type != "ALBUM" {
            return Ok(true);
        }
        let Some(album_id) = &row.album_id else {
            return Ok(false);
        };
        let found: Option<String> =
            sqlx::query_scalar("SELECT id FROM album WHERE id = ? AND deletedAt IS NULL")
                .bind(album_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(found.is_some())
    }

    async fn link_asset_ids(&self, link_id: &str) -> Result<Vec<String>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT asset.id FROM shared_link_asset
             JOIN asset ON asset.id = shared_link_asset.assetId
             WHERE shared_link_asset.sharedLinkId = ? AND asset.deletedAt IS NULL
             ORDER BY asset.fileCreatedAt ASC",
        )
        .bind(link_id)
        .fetch_all(self.pool())
        .await?)
    }

    async fn require_album_share(&self, user_id: &str, album_id: &str) -> Result<(), ApiError> {
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM album WHERE id = ? AND ownerId = ? AND deletedAt IS NULL
             UNION
             SELECT 1 FROM album
             JOIN album_user ON album_user.albumId = album.id
             WHERE album.id = ? AND album.deletedAt IS NULL AND album_user.userId = ? AND album_user.role IN ('owner', 'editor')",
        )
        .bind(album_id)
        .bind(user_id)
        .bind(album_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        if allowed.is_some() {
            Ok(())
        } else {
            Err(ApiError::text(
                StatusCode::BAD_REQUEST,
                "Not found or no album.share access",
            ))
        }
    }

    async fn require_assets(
        &self,
        user_id: &str,
        ids: &[String],
        permission: &str,
    ) -> Result<(), ApiError> {
        for id in ids {
            let owned: Option<String> =
                sqlx::query_scalar("SELECT id FROM asset WHERE id = ? AND ownerId = ?")
                    .bind(id)
                    .bind(user_id)
                    .fetch_optional(self.pool())
                    .await?;
            if owned.is_none() {
                return Err(ApiError::text(
                    StatusCode::BAD_REQUEST,
                    format!("Not found or no {permission} access"),
                ));
            }
        }
        Ok(())
    }

    async fn can_read_album(&self, user_id: &str, album_id: &str) -> Result<bool, DbError> {
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM album WHERE id = ? AND ownerId = ? AND deletedAt IS NULL
             UNION
             SELECT 1 FROM album
             JOIN album_user ON album_user.albumId = album.id
             WHERE album.id = ? AND album.deletedAt IS NULL AND album_user.userId = ?",
        )
        .bind(album_id)
        .bind(user_id)
        .bind(album_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(allowed.is_some())
    }

    async fn download_row(&self, id: &str) -> Result<Option<DownloadRow>, DbError> {
        Ok(sqlx::query_as::<_, DownloadRow>(
            "SELECT asset.id AS id, asset_exif.fileSizeInByte AS size,
                    asset.livePhotoVideoId AS live_photo_video_id,
                    asset.originalPath AS original_path,
                    asset.originalFileName AS original_file_name
             FROM asset
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.id = ? AND asset.deletedAt IS NULL",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }
}

fn viewer_from(row: &LinkRow) -> Viewer {
    Viewer {
        user_id: row.user_id.clone(),
        share: ShareGrant {
            id: row.id.clone(),
            allow_download: row.allow_download != 0,
            allow_upload: row.allow_upload != 0,
            show_metadata: row.show_exif != 0,
            album_id: row.album_id.clone(),
            link_type: row.link_type.clone(),
            password: row.password.clone(),
        },
    }
}

fn push_archive(
    archives: &mut Vec<Archive>,
    current: &mut Archive,
    id: &str,
    size: i64,
    limit: i64,
) {
    current.asset_ids.push(id.to_string());
    current.size += size;
    if current.size <= limit {
        return;
    }
    archives.push(std::mem::replace(
        current,
        Archive {
            size: 0,
            asset_ids: Vec::new(),
        },
    ));
}

fn bucket_month_sql(album: bool, partners: bool, desc: bool) -> String {
    let direction = if desc { "DESC" } else { "ASC" };
    let (from, filter) = if album {
        (
            "FROM asset JOIN album_asset ON album_asset.assetId = asset.id",
            "album_asset.albumId = ?",
        )
    } else if partners {
        ("FROM asset", PARTNER_TIMELINE)
    } else {
        ("FROM asset", "asset.ownerId = ?")
    };
    format!(
        "SELECT substr(asset.localDateTime, 1, 7) AS month, COUNT(*) AS count
         {from}
         WHERE {filter} AND asset.deletedAt IS NULL AND asset.visibility = 'timeline'
         GROUP BY month
         ORDER BY month {direction}"
    )
}

fn bucket_asset_sql(album: bool, partners: bool, desc: bool) -> String {
    let direction = if desc { "DESC" } else { "ASC" };
    let (from, filter) = if album {
        (
            "FROM asset JOIN album_asset ON album_asset.assetId = asset.id",
            "album_asset.albumId = ?",
        )
    } else if partners {
        ("FROM asset", PARTNER_TIMELINE)
    } else {
        ("FROM asset", "asset.ownerId = ?")
    };
    format!(
        "SELECT asset.id AS id, asset.duration AS duration, asset.visibility AS visibility,
                asset.isFavorite AS is_favorite, asset.type AS asset_type,
                asset.livePhotoVideoId AS live_photo_video_id,
                asset.fileCreatedAt AS file_created_at, asset.localDateTime AS local_date_time,
                asset.createdAt AS created_at, asset.ownerId AS owner_id,
                asset.width AS width, asset.height AS height, asset.thumbhash AS thumbhash,
                asset_exif.projectionType AS projection_type, asset_exif.city AS city,
                asset_exif.country AS country, asset_exif.latitude AS latitude,
                asset_exif.longitude AS longitude
         {from}
         LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
         WHERE substr(asset.localDateTime, 1, 7) = ? AND {filter}
           AND asset.deletedAt IS NULL AND asset.visibility = 'timeline'
         ORDER BY asset.localDateTime {direction}, asset.fileCreatedAt {direction},
                  asset.originalFileName {direction}"
    )
}

fn bucket_json(rows: &[BucketAsset], show_place: bool, coordinates: bool) -> Value {
    let mut ids = Vec::new();
    let mut duration = Vec::new();
    let mut visibility = Vec::new();
    let mut is_favorite = Vec::new();
    let mut is_image = Vec::new();
    let mut is_trashed = Vec::new();
    let mut live_photo = Vec::new();
    let mut file_created_at = Vec::new();
    let mut offset_hours = Vec::new();
    let mut created_at = Vec::new();
    let mut owner_id = Vec::new();
    let mut projection = Vec::new();
    let mut ratio = Vec::new();
    let mut thumbhash = Vec::new();
    let mut city = Vec::new();
    let mut country = Vec::new();
    let mut latitude = Vec::new();
    let mut longitude = Vec::new();
    for row in rows {
        ids.push(json!(row.id));
        duration.push(json!(row.duration));
        visibility.push(json!(row.visibility));
        is_favorite.push(json!(row.is_favorite != 0));
        is_image.push(json!(row.asset_type == "IMAGE"));
        is_trashed.push(json!(false));
        live_photo.push(json!(row.live_photo_video_id));
        file_created_at.push(json!(row.file_created_at));
        offset_hours.push(json!(offset_hours_between(
            &row.local_date_time,
            &row.file_created_at,
        )));
        created_at.push(json!(row.created_at));
        owner_id.push(json!(row.owner_id));
        projection.push(json!(row.projection_type));
        ratio.push(json!(aspect_ratio(row.width, row.height)));
        thumbhash.push(json!(row
            .thumbhash
            .as_ref()
            .map(|bytes| crate::media::checksum_base64(bytes))));
        if show_place {
            city.push(json!(row.city));
            country.push(json!(row.country));
        }
        if coordinates {
            latitude.push(json!(row.latitude));
            longitude.push(json!(row.longitude));
        }
    }
    let mut value = json!({
        "id": ids,
        "duration": duration,
        "visibility": visibility,
        "isFavorite": is_favorite,
        "isImage": is_image,
        "isTrashed": is_trashed,
        "livePhotoVideoId": live_photo,
        "fileCreatedAt": file_created_at,
        "localOffsetHours": offset_hours,
        "createdAt": created_at,
        "ownerId": owner_id,
        "projectionType": projection,
        "ratio": ratio,
        "thumbhash": thumbhash,
        "status": vec![json!("active"); rows.len()],
    });
    if show_place {
        value["city"] = json!(city);
        value["country"] = json!(country);
    }
    if coordinates {
        value["latitude"] = json!(latitude);
        value["longitude"] = json!(longitude);
    }
    value
}

fn aspect_ratio(width: Option<i64>, height: Option<i64>) -> f64 {
    match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            ((width as f64 / height as f64) * 1000.0).round() / 1000.0
        }
        _ => 1.0,
    }
}

fn offset_hours_between(local: &str, file_created: &str) -> f64 {
    let Ok(local) = DateTime::parse_from_rfc3339(local) else {
        return 0.0;
    };
    let Ok(file_created) = DateTime::parse_from_rfc3339(file_created) else {
        return 0.0;
    };
    (local.timestamp() - file_created.timestamp()) as f64 / 3600.0
}

/// Main timeline only. Album buckets stay on the album, and a partner is included
/// only when that recipient has turned `inTimeline` on.
const PARTNER_TIMELINE: &str = "(asset.ownerId = ? OR asset.ownerId IN (
    SELECT sharedById FROM partner WHERE sharedWithId = ? AND inTimeline = 1
))";

pub(crate) struct TimelineRequest<'a> {
    pub user_id: &'a str,
    pub grant: Option<&'a ShareGrant>,
    pub album_id: Option<&'a str>,
    pub order: &'a str,
    pub with_partners: bool,
}

fn timeline_partners(request: &TimelineRequest<'_>) -> bool {
    request.with_partners && request.album_id.is_none() && request.grant.is_none()
}

fn month_key(time_bucket: &str) -> String {
    let trimmed = time_bucket.trim().trim_start_matches('+');
    trimmed.chars().take(7).collect()
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn blank_to_null(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn optional_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn is_expired(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value)
        .map(|when| when <= Utc::now())
        .unwrap_or(false)
}

fn decode_key(token: &str) -> Option<Vec<u8>> {
    if token.len() == 100 {
        return hex::decode(token).ok().filter(|bytes| bytes.len() == 50);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .ok()
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(token).ok())
}

fn slug_taken(error: &sqlx::Error) -> bool {
    let message = error.to_string();
    message.contains("UNIQUE") && message.contains("slug")
}

fn unique_constraint(error: &sqlx::Error) -> bool {
    let message = error.to_string();
    message.contains("2067")
        || message.contains("1555")
        || message.contains("UNIQUE constraint failed")
}

fn sanitize_name(filename: &str) -> String {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_control() || character == '"' {
                '_'
            } else {
                character
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

fn unique_name(used: &mut HashMap<String, u32>, filename: &str) -> String {
    let count = used.entry(filename.to_string()).or_insert(0);
    let current = *count;
    *count += 1;
    if current == 0 {
        return filename.to_string();
    }
    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(filename);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    format!("{stem}+{current}{extension}")
}

fn write_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut central = Vec::new();
    for (name, bytes) in entries {
        let name_bytes = name.as_bytes();
        let offset = u32::try_from(body.len()).unwrap_or(u32::MAX);
        let crc = crc32(bytes);
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let name_len = u16::try_from(name_bytes.len()).unwrap_or(u16::MAX);
        body.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        body.extend_from_slice(&20_u16.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&name_len.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(name_bytes);
        body.extend_from_slice(bytes);
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_offset = u32::try_from(body.len()).unwrap_or(u32::MAX);
    let central_size = u32::try_from(central.len()).unwrap_or(u32::MAX);
    let count = u16::try_from(entries.len()).unwrap_or(u16::MAX);
    body.extend_from_slice(&central);
    body.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body.extend_from_slice(&central_size.to_le_bytes());
    body.extend_from_slice(&central_offset.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
