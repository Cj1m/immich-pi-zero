//! Albums are removed by deleting the row. The assets stay.

use serde_json::{json, Value};

use crate::db::{avatar_color_for_email, new_id, new_update_id, now_iso, App, DbError};

#[derive(sqlx::FromRow)]
struct AlbumRow {
    id: String,
    #[allow(dead_code)]
    owner_id: String,
    album_name: String,
    description: String,
    created_at: String,
    updated_at: String,
    album_thumbnail_asset_id: Option<String>,
    is_activity_enabled: i64,
    album_order: String,
    #[allow(dead_code)]
    update_id: String,
    asset_count: i64,
}

#[derive(sqlx::FromRow)]
struct AlbumUserRow {
    role: String,
    id: String,
    email: String,
    name: String,
    profile_image_path: String,
    avatar_color: Option<String>,
    profile_changed_at: String,
}

const ALBUM_SQL: &str = "SELECT
    album.id AS id,
    album.ownerId AS owner_id,
    album.albumName AS album_name,
    album.description AS description,
    album.createdAt AS created_at,
    album.updatedAt AS updated_at,
    album.albumThumbnailAssetId AS album_thumbnail_asset_id,
    album.isActivityEnabled AS is_activity_enabled,
    album.albumOrder AS album_order,
    album.updateId AS update_id,
    (SELECT COUNT(*) FROM album_asset WHERE album_asset.albumId = album.id) AS asset_count
FROM album";

impl App {
    pub async fn create_album(
        &self,
        owner_id: &str,
        name: &str,
        description: Option<&str>,
        asset_ids: &[String],
    ) -> Result<Value, DbError> {
        let id = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        let description = description.unwrap_or("");
        sqlx::query(
            "INSERT INTO album (
                id, ownerId, albumName, description, createdAt, updatedAt,
                albumThumbnailAssetId, isActivityEnabled, albumOrder, updateId
            ) VALUES (?, ?, ?, ?, ?, ?, NULL, 1, 'desc', ?)",
        )
        .bind(&id)
        .bind(owner_id)
        .bind(name)
        .bind(description)
        .bind(&now)
        .bind(&now)
        .bind(&update_id)
        .execute(self.pool())
        .await?;
        sqlx::query(
            "INSERT INTO album_user (albumId, userId, role, updateId) VALUES (?, ?, 'owner', ?)",
        )
        .bind(&id)
        .bind(owner_id)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        for asset_id in asset_ids {
            let _ = self.link_asset(owner_id, &id, asset_id).await?;
        }
        self.album_json(owner_id, &id).await
    }

    pub async fn list_albums(&self, user_id: &str) -> Result<Value, DbError> {
        let sql = format!(
            "{ALBUM_SQL} WHERE album.deletedAt IS NULL AND (album.ownerId = ? OR album.id IN (SELECT albumId FROM album_user WHERE userId = ?)) ORDER BY album.createdAt"
        );
        let rows = sqlx::query_as::<_, AlbumRow>(&sql)
            .bind(user_id)
            .bind(user_id)
            .fetch_all(self.pool())
            .await?;
        let mut albums = Vec::new();
        for row in rows {
            albums.push(self.album_value(&row).await?);
        }
        Ok(Value::Array(albums))
    }

    pub async fn album_json(&self, user_id: &str, id: &str) -> Result<Value, DbError> {
        let row = self.album_row(user_id, id).await?;
        self.album_value(&row).await
    }

    pub async fn patch_album(
        &self,
        owner_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        self.require_owner(owner_id, id).await?;
        let mut row = self.album_row_by_id(id).await?;
        if let Some(name) = body.get("albumName").and_then(|value| value.as_str()) {
            row.album_name = name.to_string();
        }
        if body.get("description").is_some() {
            row.description = body["description"].as_str().unwrap_or("").to_string();
        }
        if let Some(order) = body.get("order").and_then(|value| value.as_str()) {
            if !matches!(order, "asc" | "desc") {
                return Err(DbError::Validation);
            }
            row.album_order = order.to_string();
        }
        if let Some(flag) = body
            .get("isActivityEnabled")
            .and_then(|value| value.as_bool())
        {
            row.is_activity_enabled = i64::from(flag);
        }
        if body.get("albumThumbnailAssetId").is_some() {
            row.album_thumbnail_asset_id =
                body["albumThumbnailAssetId"].as_str().map(str::to_string);
        }
        sqlx::query(
            "UPDATE album SET albumName = ?, description = ?, albumOrder = ?, isActivityEnabled = ?,
             albumThumbnailAssetId = ?, updatedAt = ?, updateId = ? WHERE id = ?",
        )
        .bind(&row.album_name)
        .bind(&row.description)
        .bind(&row.album_order)
        .bind(row.is_activity_enabled)
        .bind(&row.album_thumbnail_asset_id)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(id)
        .execute(self.pool())
        .await?;
        self.album_json(owner_id, id).await
    }

    pub async fn delete_album(&self, owner_id: &str, id: &str) -> Result<(), DbError> {
        self.require_owner(owner_id, id).await?;
        let assets: Vec<String> =
            sqlx::query_scalar("SELECT assetId FROM album_asset WHERE albumId = ?")
                .bind(id)
                .fetch_all(self.pool())
                .await?;
        for asset_id in assets {
            sqlx::query(
                "INSERT INTO album_asset_audit (id, albumId, assetId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(new_id())
            .bind(id)
            .bind(asset_id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        }
        let users: Vec<String> =
            sqlx::query_scalar("SELECT userId FROM album_user WHERE albumId = ?")
                .bind(id)
                .fetch_all(self.pool())
                .await?;
        for user_id in users {
            sqlx::query(
                "INSERT INTO album_user_audit (id, albumId, userId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(new_id())
            .bind(id)
            .bind(user_id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        }
        sqlx::query("INSERT INTO album_audit (id, albumId, ownerId, updateId) VALUES (?, ?, ?, ?)")
            .bind(new_id())
            .bind(id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        sqlx::query("DELETE FROM album WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Hard-delete every album this User owns, and tell each member.
    pub(crate) async fn purge_owned_albums(&self, owner_id: &str) -> Result<(), DbError> {
        let albums: Vec<String> = sqlx::query_scalar("SELECT id FROM album WHERE ownerId = ?")
            .bind(owner_id)
            .fetch_all(self.pool())
            .await?;
        for album_id in albums {
            let users: Vec<String> =
                sqlx::query_scalar("SELECT userId FROM album_user WHERE albumId = ?")
                    .bind(&album_id)
                    .fetch_all(self.pool())
                    .await?;
            for user_id in &users {
                sqlx::query(
                    "INSERT INTO album_user_audit (id, albumId, userId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(new_id())
                .bind(&album_id)
                .bind(user_id)
                .bind(owner_id)
                .bind(new_update_id())
                .execute(self.pool())
                .await?;
                // `ownerId` on this audit is the sync recipient, matching v3.3.6 `album_audit.userId`.
                sqlx::query(
                    "INSERT INTO album_audit (id, albumId, ownerId, updateId) VALUES (?, ?, ?, ?)",
                )
                .bind(new_id())
                .bind(&album_id)
                .bind(user_id)
                .bind(new_update_id())
                .execute(self.pool())
                .await?;
            }
            sqlx::query("DELETE FROM album WHERE id = ?")
                .bind(&album_id)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    pub async fn add_album_assets(
        &self,
        owner_id: &str,
        album_id: &str,
        asset_ids: &[String],
    ) -> Result<Value, DbError> {
        self.require_owner(owner_id, album_id).await?;
        let mut results = Vec::new();
        for asset_id in asset_ids {
            results.push(self.link_asset(owner_id, album_id, asset_id).await?);
        }
        Ok(Value::Array(results))
    }

    pub async fn remove_album_assets(
        &self,
        owner_id: &str,
        album_id: &str,
        asset_ids: &[String],
    ) -> Result<Value, DbError> {
        self.require_owner(owner_id, album_id).await?;
        let mut results = Vec::new();
        for asset_id in asset_ids {
            let removed = sqlx::query("DELETE FROM album_asset WHERE albumId = ? AND assetId = ?")
                .bind(album_id)
                .bind(asset_id)
                .execute(self.pool())
                .await?;
            if removed.rows_affected() == 0 {
                results.push(json!({ "id": asset_id, "success": false, "error": "not_found" }));
                continue;
            }
            sqlx::query("DELETE FROM activity WHERE albumId = ? AND assetId = ?")
                .bind(album_id)
                .bind(asset_id)
                .execute(self.pool())
                .await?;
            sqlx::query(
                "INSERT INTO album_asset_audit (id, albumId, assetId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(new_id())
            .bind(album_id)
            .bind(asset_id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
            self.touch_album(album_id).await?;
            results.push(json!({ "id": asset_id, "success": true }));
        }
        Ok(Value::Array(results))
    }

    pub async fn add_album_users(
        &self,
        owner_id: &str,
        album_id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        self.require_owner(owner_id, album_id).await?;
        let users = body
            .get("albumUsers")
            .and_then(|value| value.as_array())
            .ok_or(DbError::Validation)?;
        for user in users {
            let user_id = user
                .get("userId")
                .and_then(|value| value.as_str())
                .ok_or(DbError::Validation)?;
            let exists: Option<String> =
                sqlx::query_scalar("SELECT id FROM user WHERE id = ? AND deletedAt IS NULL")
                    .bind(user_id)
                    .fetch_optional(self.pool())
                    .await?;
            if exists.is_none() {
                return Err(DbError::BadRequest("Invalid user"));
            }
            let role = user
                .get("role")
                .and_then(|value| value.as_str())
                .unwrap_or("editor");
            let role = if role == "viewer" { "viewer" } else { "editor" };
            if user_id == owner_id {
                continue;
            }
            sqlx::query(
                "INSERT INTO album_user (albumId, userId, role, updateId) VALUES (?, ?, ?, ?)
                 ON CONFLICT(albumId, userId) DO UPDATE SET role = excluded.role, updateId = excluded.updateId",
            )
            .bind(album_id)
            .bind(user_id)
            .bind(role)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        }
        self.touch_album(album_id).await?;
        self.album_json(owner_id, album_id).await
    }

    pub async fn remove_album_user(
        &self,
        owner_id: &str,
        album_id: &str,
        user_id: &str,
    ) -> Result<(), DbError> {
        self.require_owner(owner_id, album_id).await?;
        let target = if user_id == "me" { owner_id } else { user_id };
        if target == owner_id {
            return Err(DbError::Validation);
        }
        let removed = sqlx::query(
            "DELETE FROM album_user WHERE albumId = ? AND userId = ? AND role != 'owner'",
        )
        .bind(album_id)
        .bind(target)
        .execute(self.pool())
        .await?;
        if removed.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        sqlx::query(
            "INSERT INTO album_user_audit (id, albumId, userId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(new_id())
        .bind(album_id)
        .bind(target)
        .bind(owner_id)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn link_asset(
        &self,
        owner_id: &str,
        album_id: &str,
        asset_id: &str,
    ) -> Result<Value, DbError> {
        let owned: Option<String> =
            sqlx::query_scalar("SELECT id FROM asset WHERE id = ? AND ownerId = ?")
                .bind(asset_id)
                .bind(owner_id)
                .fetch_optional(self.pool())
                .await?;
        if owned.is_none() {
            return Ok(json!({ "id": asset_id, "success": false, "error": "not_found" }));
        }
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
            if is_unique(&error) {
                return Ok(json!({ "id": asset_id, "success": false, "error": "duplicate" }));
            }
            return Err(error.into());
        }
        sqlx::query(
            "UPDATE album SET albumThumbnailAssetId = COALESCE(albumThumbnailAssetId, ?), updatedAt = ?, updateId = ?
             WHERE id = ?",
        )
        .bind(asset_id)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(album_id)
        .execute(self.pool())
        .await?;
        Ok(json!({ "id": asset_id, "success": true }))
    }

    async fn album_value(&self, row: &AlbumRow) -> Result<Value, DbError> {
        let users = sqlx::query_as::<_, AlbumUserRow>(
            "SELECT album_user.role AS role, user.id AS id, user.email AS email, user.name AS name,
                    user.profileImagePath AS profile_image_path, user.avatarColor AS avatar_color,
                    user.profileChangedAt AS profile_changed_at
             FROM album_user JOIN user ON user.id = album_user.userId
             WHERE album_user.albumId = ?
             ORDER BY CASE album_user.role WHEN 'owner' THEN 0 ELSE 1 END, user.name",
        )
        .bind(&row.id)
        .fetch_all(self.pool())
        .await?;
        let album_users: Vec<Value> = users
            .iter()
            .map(|user| {
                let color = user
                    .avatar_color
                    .clone()
                    .unwrap_or_else(|| avatar_color_for_email(&user.email).to_string());
                json!({
                    "role": user.role,
                    "user": {
                        "id": user.id,
                        "email": user.email,
                        "name": user.name,
                        "profileImagePath": user.profile_image_path,
                        "avatarColor": color,
                        "profileChangedAt": user.profile_changed_at,
                    }
                })
            })
            .collect();
        let shared = album_users.len() > 1;
        let linked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shared_link WHERE albumId = ?")
            .bind(&row.id)
            .fetch_one(self.pool())
            .await?;
        Ok(json!({
            "id": row.id,
            "albumName": row.album_name,
            "description": row.description,
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "albumThumbnailAssetId": row.album_thumbnail_asset_id,
            "shared": shared,
            "albumUsers": album_users,
            "hasSharedLink": linked > 0,
            "assetCount": row.asset_count,
            "isActivityEnabled": row.is_activity_enabled != 0,
            "order": row.album_order,
        }))
    }

    async fn album_row(&self, user_id: &str, id: &str) -> Result<AlbumRow, DbError> {
        let sql = format!(
            "{ALBUM_SQL} WHERE album.deletedAt IS NULL AND album.id = ? AND (album.ownerId = ? OR album.id IN (SELECT albumId FROM album_user WHERE userId = ?))"
        );
        sqlx::query_as::<_, AlbumRow>(&sql)
            .bind(id)
            .bind(user_id)
            .bind(user_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn album_row_by_id(&self, id: &str) -> Result<AlbumRow, DbError> {
        let sql = format!("{ALBUM_SQL} WHERE album.deletedAt IS NULL AND album.id = ?");
        sqlx::query_as::<_, AlbumRow>(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn require_owner(&self, owner_id: &str, album_id: &str) -> Result<(), DbError> {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM album WHERE id = ? AND ownerId = ? AND deletedAt IS NULL",
        )
        .bind(album_id)
        .bind(owner_id)
        .fetch_optional(self.pool())
        .await?;
        id.map(|_| ()).ok_or(DbError::NotFound)
    }

    async fn touch_album(&self, album_id: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE album SET updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(now_iso())
            .bind(new_update_id())
            .bind(album_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }
}

fn is_unique(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database) = error else {
        return false;
    };
    database
        .code()
        .as_deref()
        .is_some_and(|code| code == "2067" || code == "1555")
        || database.message().contains("UNIQUE constraint failed")
}
