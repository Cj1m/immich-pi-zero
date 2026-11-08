//! API keys, the caller's sessions, profile image, notifications, and usage counts.
//! A session may grant any permission. An API key may grant only what it holds.
//! Creating a notification tells that user. No mail is sent.

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};

use crate::db::{new_id, new_update_id, now_iso, random_token, sha256, App, DbError};
use crate::thumbs::{write_profile_jpeg, ProfileJpeg};

const PERMISSIONS: &[&str] = &[
    "all",
    "activity.create",
    "activity.read",
    "activity.update",
    "activity.delete",
    "activity.statistics",
    "apiKey.create",
    "apiKey.read",
    "apiKey.update",
    "apiKey.delete",
    "apiKey.rotate",
    "asset.read",
    "asset.update",
    "asset.delete",
    "asset.statistics",
    "asset.share",
    "asset.view",
    "asset.download",
    "asset.upload",
    "asset.copy",
    "asset.derive",
    "assetFile.read",
    "assetFile.delete",
    "assetFile.download",
    "asset.edit.get",
    "asset.edit.create",
    "asset.edit.delete",
    "album.create",
    "album.read",
    "album.update",
    "album.delete",
    "album.statistics",
    "album.share",
    "album.download",
    "albumAsset.create",
    "albumAsset.delete",
    "albumUser.create",
    "albumUser.update",
    "albumUser.delete",
    "auth.changePassword",
    "authDevice.delete",
    "archive.read",
    "backup.list",
    "backup.download",
    "backup.upload",
    "backup.delete",
    "clusterGroup.read",
    "clusterGroup.leave",
    "clusterGroupRequest.create",
    "clusterGroupRequest.read",
    "clusterGroupRequest.delete",
    "adminConfig.read",
    "adminConfig.update",
    "userConfig.read",
    "duplicate.read",
    "duplicate.delete",
    "face.create",
    "face.read",
    "face.update",
    "face.delete",
    "folder.read",
    "job.create",
    "job.read",
    "library.create",
    "library.read",
    "library.update",
    "library.delete",
    "library.statistics",
    "timeline.read",
    "timeline.download",
    "maintenance",
    "map.read",
    "map.search",
    "memory.create",
    "memory.read",
    "memory.update",
    "memory.delete",
    "memory.statistics",
    "memoryAsset.create",
    "memoryAsset.delete",
    "notification.create",
    "notification.read",
    "notification.update",
    "notification.delete",
    "partner.create",
    "partner.read",
    "partner.update",
    "partner.delete",
    "person.create",
    "person.read",
    "person.update",
    "person.delete",
    "person.statistics",
    "person.merge",
    "person.reassign",
    "pinCode.create",
    "pinCode.update",
    "pinCode.delete",
    "plugin.create",
    "plugin.read",
    "plugin.update",
    "plugin.delete",
    "server.about",
    "server.apkLinks",
    "server.storage",
    "server.statistics",
    "server.versionCheck",
    "serverLicense.read",
    "serverLicense.update",
    "serverLicense.delete",
    "session.create",
    "session.read",
    "session.update",
    "session.delete",
    "session.lock",
    "sharedLink.create",
    "sharedLink.read",
    "sharedLink.update",
    "sharedLink.delete",
    "stack.create",
    "stack.read",
    "stack.update",
    "stack.delete",
    "sync.stream",
    "syncCheckpoint.read",
    "syncCheckpoint.update",
    "syncCheckpoint.delete",
    "systemConfig.read",
    "systemConfig.update",
    "systemMetadata.read",
    "systemMetadata.update",
    "tag.create",
    "tag.read",
    "tag.update",
    "tag.delete",
    "tag.asset",
    "user.read",
    "user.update",
    "userLicense.create",
    "userLicense.read",
    "userLicense.update",
    "userLicense.delete",
    "userOnboarding.read",
    "userOnboarding.update",
    "userOnboarding.delete",
    "userPreference.read",
    "userPreference.update",
    "userProfileImage.create",
    "userProfileImage.read",
    "userProfileImage.update",
    "userProfileImage.delete",
    "queue.read",
    "queue.update",
    "queueJob.create",
    "queueJob.read",
    "queueJob.update",
    "queueJob.delete",
    "workflow.create",
    "workflow.read",
    "workflow.update",
    "workflow.delete",
    "workflow.logs",
    "adminUser.create",
    "adminUser.read",
    "adminUser.update",
    "adminUser.delete",
    "adminSession.read",
    "adminAuth.unlinkAll",
];

pub(crate) enum AccountFail {
    Db(DbError),
    BadRequest(&'static str),
    Unauthorized(&'static str),
}

impl From<DbError> for AccountFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for AccountFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

#[derive(Debug)]
pub(crate) struct ApiKeyAuth {
    pub user_id: String,
    pub permissions: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: String,
    name: String,
    created_at: String,
    updated_at: String,
    permissions: String,
}

#[derive(sqlx::FromRow)]
struct SessionListRow {
    id: String,
    created_at: String,
    updated_at: String,
    expires_at: Option<String>,
    device_type: String,
    device_os: String,
    app_version: Option<String>,
    is_pending_sync_reset: i64,
}

#[derive(sqlx::FromRow)]
struct NotificationRow {
    id: String,
    created_at: String,
    level: String,
    #[sqlx(rename = "type")]
    kind: String,
    title: String,
    description: Option<String>,
    data: Option<String>,
    read_at: Option<String>,
}

impl App {
    pub(crate) async fn lookup_api_key(&self, token: &str) -> Result<Option<ApiKeyAuth>, DbError> {
        let hashed = sha256(token.as_bytes());
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT api_key.userId, api_key.permissions
             FROM api_key
             JOIN user ON user.id = api_key.userId AND user.deletedAt IS NULL
             WHERE api_key.key = ?",
        )
        .bind(hashed)
        .fetch_optional(self.pool())
        .await?;
        let Some((user_id, permissions)) = row else {
            return Ok(None);
        };
        let permissions = serde_json::from_str(&permissions).unwrap_or_default();
        Ok(Some(ApiKeyAuth {
            user_id,
            permissions,
        }))
    }

    pub(crate) async fn list_api_keys(&self, user_id: &str) -> Result<Value, AccountFail> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, createdAt AS created_at, updatedAt AS updated_at, permissions
             FROM api_key WHERE userId = ? ORDER BY createdAt DESC, id",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        let keys = rows.iter().map(api_key_json).collect::<Vec<_>>();
        Ok(Value::Array(keys))
    }

    pub(crate) async fn create_api_key(
        &self,
        user_id: &str,
        held: Option<&[String]>,
        body: &Value,
    ) -> Result<Value, AccountFail> {
        let name = create_name(body)?;
        let permissions = required_permissions(body)?;
        if let Some(held) = held {
            if !grants(held, &permissions) {
                return Err(AccountFail::BadRequest(
                    "Cannot grant permissions you do not have",
                ));
            }
        }
        let secret = random_token();
        let now = now_iso();
        let id = new_id();
        let stored = serde_json::to_string(&permissions)
            .map_err(|error| DbError::Store(error.to_string()))?;
        sqlx::query(
            "INSERT INTO api_key (
                id, name, key, userId, createdAt, updatedAt, permissions, updateId
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&name)
        .bind(sha256(secret.as_bytes()))
        .bind(user_id)
        .bind(&now)
        .bind(&now)
        .bind(&stored)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        let api_key = json!({
            "id": id,
            "name": name,
            "createdAt": now,
            "updatedAt": now,
            "permissions": permissions,
        });
        Ok(json!({
            "id": api_key["id"],
            "name": api_key["name"],
            "createdAt": api_key["createdAt"],
            "updatedAt": api_key["updatedAt"],
            "permissions": api_key["permissions"],
            "secret": secret,
            "apiKey": api_key,
        }))
    }

    pub(crate) async fn update_api_key(
        &self,
        user_id: &str,
        id: &str,
        held: Option<&[String]>,
        body: &Value,
    ) -> Result<Value, AccountFail> {
        if !body.is_object() {
            return Err(AccountFail::BadRequest("Validation failed"));
        }
        let name = optional_string(body, "name")?;
        let permissions = optional_permissions(body)?;
        let existing = self.api_key_row(user_id, id).await?;
        let Some(existing) = existing else {
            return Err(AccountFail::BadRequest("API Key not found"));
        };
        if let (Some(held), Some(permissions)) = (held, permissions.as_ref()) {
            if !grants(held, permissions) {
                return Err(AccountFail::BadRequest(
                    "Cannot grant permissions you do not have",
                ));
            }
        }
        let name = name.unwrap_or(existing.name);
        let permissions = match permissions {
            Some(permissions) => permissions,
            None => serde_json::from_str(&existing.permissions).unwrap_or_default(),
        };
        let stored = serde_json::to_string(&permissions)
            .map_err(|error| DbError::Store(error.to_string()))?;
        let now = now_iso();
        sqlx::query(
            "UPDATE api_key
             SET name = ?, permissions = ?, updatedAt = ?, updateId = ?
             WHERE id = ? AND userId = ?",
        )
        .bind(&name)
        .bind(&stored)
        .bind(&now)
        .bind(new_update_id())
        .bind(id)
        .bind(user_id)
        .execute(self.pool())
        .await?;
        Ok(json!({
            "id": id,
            "name": name,
            "createdAt": existing.created_at,
            "updatedAt": now,
            "permissions": permissions,
        }))
    }

    pub(crate) async fn rotate_api_key(
        &self,
        user_id: &str,
        id: &str,
        held: Option<&[String]>,
    ) -> Result<Value, AccountFail> {
        let existing = self.api_key_row(user_id, id).await?;
        let Some(existing) = existing else {
            return Err(AccountFail::BadRequest("API Key not found"));
        };
        let permissions: Vec<String> =
            serde_json::from_str(&existing.permissions).unwrap_or_default();
        if let Some(held) = held {
            if !grants(held, &permissions) {
                return Err(AccountFail::BadRequest(
                    "Cannot rotate an API Key with permissions you do not have",
                ));
            }
        }
        let secret = random_token();
        let now = now_iso();
        sqlx::query(
            "UPDATE api_key SET key = ?, updatedAt = ?, updateId = ? WHERE id = ? AND userId = ?",
        )
        .bind(sha256(secret.as_bytes()))
        .bind(&now)
        .bind(new_update_id())
        .bind(id)
        .bind(user_id)
        .execute(self.pool())
        .await?;
        let api_key = json!({
            "id": existing.id,
            "name": existing.name,
            "createdAt": existing.created_at,
            "updatedAt": now,
            "permissions": permissions,
        });
        Ok(json!({
            "id": api_key["id"],
            "name": api_key["name"],
            "createdAt": api_key["createdAt"],
            "updatedAt": api_key["updatedAt"],
            "permissions": api_key["permissions"],
            "secret": secret,
            "apiKey": api_key,
        }))
    }

    pub(crate) async fn delete_api_key(&self, user_id: &str, id: &str) -> Result<(), AccountFail> {
        let existing = self.api_key_row(user_id, id).await?;
        if existing.is_none() {
            return Err(AccountFail::BadRequest("API Key not found"));
        }
        sqlx::query("DELETE FROM api_key WHERE id = ? AND userId = ?")
            .bind(id)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn my_sessions(
        &self,
        user_id: &str,
        current_id: Option<&str>,
    ) -> Result<Value, AccountFail> {
        let now = now_iso();
        let rows = sqlx::query_as::<_, SessionListRow>(
            "SELECT session.id AS id,
                    session.createdAt AS created_at,
                    session.updatedAt AS updated_at,
                    session.expiresAt AS expires_at,
                    session.deviceType AS device_type,
                    session.deviceOS AS device_os,
                    session.appVersion AS app_version,
                    session.isPendingSyncReset AS is_pending_sync_reset
             FROM session
             JOIN user ON user.id = session.userId AND user.deletedAt IS NULL
             WHERE session.userId = ?
               AND (session.expiresAt IS NULL OR session.expiresAt > ?)
             ORDER BY session.updatedAt DESC, session.createdAt DESC",
        )
        .bind(user_id)
        .bind(&now)
        .fetch_all(self.pool())
        .await?;
        let sessions = rows
            .iter()
            .map(|row| session_json(row, current_id))
            .collect::<Vec<_>>();
        Ok(Value::Array(sessions))
    }

    pub(crate) async fn delete_my_session(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<(), AccountFail> {
        let owned: Option<String> =
            sqlx::query_scalar("SELECT id FROM session WHERE id = ? AND userId = ?")
                .bind(id)
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        if owned.is_none() {
            return Err(AccountFail::BadRequest(
                "Not found or no authDevice.delete access",
            ));
        }
        sqlx::query("DELETE FROM session WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// `current_id` stays. An API-key caller has no session, so every row goes.
    pub(crate) async fn delete_other_sessions(
        &self,
        user_id: &str,
        current_id: Option<&str>,
    ) -> Result<(), AccountFail> {
        sqlx::query("DELETE FROM session WHERE userId = ? AND (? IS NULL OR id != ?)")
            .bind(user_id)
            .bind(current_id)
            .bind(current_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn set_profile_image(
        &self,
        user_id: &str,
        source: &Path,
    ) -> Result<Value, AccountFail> {
        let data_dir = self.data_dir().to_path_buf();
        let owner = user_id.to_string();
        let source_path = source.to_path_buf();
        let written = tokio::task::spawn_blocking(move || {
            write_profile_jpeg(&data_dir, &owner, &source_path)
        })
        .await
        .map_err(|error| DbError::Store(error.to_string()))?;
        let written = match written {
            Ok(ProfileJpeg::Wrote(path)) => path,
            Ok(ProfileJpeg::Undecodable) => {
                let _ = tokio::fs::remove_file(source).await;
                return Err(AccountFail::BadRequest("Unable to process profile image"));
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(source).await;
                return Err(DbError::Store(error.to_string()).into());
            }
        };
        let old: Option<String> = sqlx::query_scalar(
            "SELECT profileImagePath FROM user WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(old) = old else {
            let _ = tokio::fs::remove_file(&written).await;
            let _ = tokio::fs::remove_file(source).await;
            return Err(AccountFail::BadRequest("User not found"));
        };
        let path = written.to_string_lossy().into_owned();
        let now = now_iso();
        let saved = sqlx::query(
            "UPDATE user
             SET profileImagePath = ?, profileChangedAt = ?, updatedAt = ?, updateId = ?
             WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(&path)
        .bind(&now)
        .bind(&now)
        .bind(new_update_id())
        .bind(user_id)
        .execute(self.pool())
        .await;
        let _ = tokio::fs::remove_file(source).await;
        if let Err(error) = saved {
            let _ = tokio::fs::remove_file(&written).await;
            return Err(error.into());
        }
        if !old.is_empty() && old != path {
            remove_profile_file(self.data_dir(), &old).await;
        }
        Ok(json!({
            "userId": user_id,
            "profileImagePath": path,
            "profileChangedAt": now,
        }))
    }

    pub(crate) async fn list_notifications(
        &self,
        user_id: &str,
        unread: Option<bool>,
    ) -> Result<Value, AccountFail> {
        let rows = if unread == Some(true) {
            sqlx::query_as::<_, NotificationRow>(
                "SELECT id, createdAt AS created_at, level, type, title, description, data, readAt AS read_at
                 FROM notification
                 WHERE userId = ? AND deletedAt IS NULL AND readAt IS NULL
                 ORDER BY createdAt DESC, id",
            )
            .bind(user_id)
            .fetch_all(self.pool())
            .await?
        } else {
            sqlx::query_as::<_, NotificationRow>(
                "SELECT id, createdAt AS created_at, level, type, title, description, data, readAt AS read_at
                 FROM notification
                 WHERE userId = ? AND deletedAt IS NULL
                 ORDER BY createdAt DESC, id",
            )
            .bind(user_id)
            .fetch_all(self.pool())
            .await?
        };
        Ok(Value::Array(rows.iter().map(notification_json).collect()))
    }

    pub(crate) async fn update_notifications(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<(), AccountFail> {
        let ids = notification_ids(body)?;
        let read_at = required_read_at(body)?;
        self.require_notifications(user_id, &ids).await?;
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE notification
             SET readAt = ?, updatedAt = ?, updateId = ?
             WHERE userId = ? AND id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql)
            .bind(&read_at)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(user_id);
        for id in &ids {
            query = query.bind(id);
        }
        query.execute(self.pool()).await?;
        Ok(())
    }

    pub(crate) async fn update_notification(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, AccountFail> {
        if !body.is_object() {
            return Err(AccountFail::BadRequest("Validation failed"));
        }
        self.require_notifications(user_id, &[id.to_string()])
            .await?;
        let read_at = match body.get("readAt") {
            None => None,
            Some(value) => Some(parse_read_at(value)?),
        };
        if let Some(read_at) = read_at {
            sqlx::query(
                "UPDATE notification
                 SET readAt = ?, updatedAt = ?, updateId = ?
                 WHERE id = ? AND userId = ? AND deletedAt IS NULL",
            )
            .bind(&read_at)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(id)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        }
        let row = self.notification_row(id).await?;
        let Some(row) = row else {
            return Err(AccountFail::BadRequest(
                "Not found or no notification.update access",
            ));
        };
        Ok(notification_json(&row))
    }

    /// Writes one in-app notification and tells that user's open socket. No mail is sent.
    #[doc(hidden)]
    pub async fn create_notification(
        &self,
        user_id: &str,
        title: &str,
        description: Option<&str>,
        data: Option<Value>,
    ) -> Result<Value, DbError> {
        let now = now_iso();
        let id = new_id();
        let stored = match &data {
            Some(value) if !value.is_null() => Some(
                serde_json::to_string(value).map_err(|error| DbError::Store(error.to_string()))?,
            ),
            _ => None,
        };
        sqlx::query(
            "INSERT INTO notification (
                id, createdAt, updatedAt, deletedAt, updateId, userId, level, type,
                data, title, description, readAt
             ) VALUES (?, ?, ?, NULL, ?, ?, 'info', 'info', ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&now)
        .bind(&now)
        .bind(new_update_id())
        .bind(user_id)
        .bind(&stored)
        .bind(title)
        .bind(description)
        .execute(self.pool())
        .await?;
        let row = NotificationRow {
            id,
            created_at: now,
            level: "info".to_string(),
            kind: "info".to_string(),
            title: title.to_string(),
            description: description.map(str::to_string),
            data: stored,
            read_at: None,
        };
        let document = notification_json(&row);
        let packet = format!("42{}", json!(["on_notification", document]));
        self.send_user_event(user_id, &packet);
        Ok(document)
    }

    pub(crate) async fn asset_statistics(
        &self,
        user_id: &str,
        query: &str,
    ) -> Result<Value, AccountFail> {
        let pairs = query_pairs(query);
        let visibility = match pair(&pairs, "visibility").as_deref() {
            None => None,
            Some("timeline" | "archive" | "hidden") => pair(&pairs, "visibility"),
            Some("locked") => {
                return Err(AccountFail::Unauthorized("Elevated permission is required"));
            }
            Some(_) => return Err(AccountFail::BadRequest("Validation failed")),
        };
        let favorite = optional_bool(&pairs, "isFavorite")?;
        let trashed = optional_bool(&pairs, "isTrashed")?.unwrap_or(false);
        let mut sql = String::from(
            "SELECT
                COALESCE(SUM(CASE WHEN type = 'IMAGE' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'VIDEO' THEN 1 ELSE 0 END), 0),
                COUNT(*)
             FROM asset WHERE ownerId = ?",
        );
        if trashed {
            sql.push_str(" AND deletedAt IS NOT NULL AND status != 'deleted'");
        } else {
            sql.push_str(" AND deletedAt IS NULL");
        }
        if visibility.is_some() {
            sql.push_str(" AND visibility = ?");
        } else {
            sql.push_str(" AND visibility IN ('timeline', 'archive')");
        }
        if favorite.is_some() {
            sql.push_str(" AND isFavorite = ?");
        }
        let mut query = sqlx::query_as::<_, (i64, i64, i64)>(&sql).bind(user_id);
        if let Some(visibility) = &visibility {
            query = query.bind(visibility);
        }
        if let Some(favorite) = favorite {
            query = query.bind(i64::from(favorite));
        }
        let (images, videos, total) = query.fetch_one(self.pool()).await?;
        Ok(json!({ "images": images, "videos": videos, "total": total }))
    }

    async fn api_key_row(&self, user_id: &str, id: &str) -> Result<Option<ApiKeyRow>, AccountFail> {
        Ok(sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, createdAt AS created_at, updatedAt AS updated_at, permissions
             FROM api_key WHERE id = ? AND userId = ?",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?)
    }

    async fn require_notifications(
        &self,
        user_id: &str,
        ids: &[String],
    ) -> Result<(), AccountFail> {
        if ids.is_empty() {
            return Err(AccountFail::BadRequest("Validation failed"));
        }
        let unique = ids.iter().collect::<std::collections::HashSet<_>>().len();
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("SELECT id FROM notification WHERE userId = ? AND id IN ({placeholders})");
        let mut query = sqlx::query_scalar::<_, String>(&sql).bind(user_id);
        for id in ids {
            query = query.bind(id);
        }
        let found = query.fetch_all(self.pool()).await?;
        let found_unique = found.iter().collect::<std::collections::HashSet<_>>().len();
        if found_unique != unique {
            return Err(AccountFail::BadRequest(
                "Not found or no notification.update access",
            ));
        }
        Ok(())
    }

    async fn notification_row(&self, id: &str) -> Result<Option<NotificationRow>, AccountFail> {
        Ok(sqlx::query_as::<_, NotificationRow>(
            "SELECT id, createdAt AS created_at, level, type, title, description, data, readAt AS read_at
             FROM notification WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }
}

pub(crate) fn grants(held: &[String], requested: &[String]) -> bool {
    if held.iter().any(|permission| permission == "all") {
        return true;
    }
    requested
        .iter()
        .all(|permission| held.iter().any(|have| have == permission))
}

pub(crate) fn holds(held: &[String], permission: &str) -> bool {
    held.iter().any(|have| have == "all" || have == permission)
}

fn api_key_json(row: &ApiKeyRow) -> Value {
    let permissions: Vec<String> = serde_json::from_str(&row.permissions).unwrap_or_default();
    json!({
        "id": row.id,
        "name": row.name,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
        "permissions": permissions,
    })
}

fn session_json(row: &SessionListRow, current_id: Option<&str>) -> Value {
    let mut document = Map::new();
    document.insert("id".to_string(), json!(row.id));
    document.insert("createdAt".to_string(), json!(row.created_at));
    document.insert("updatedAt".to_string(), json!(row.updated_at));
    if let Some(expires_at) = &row.expires_at {
        document.insert("expiresAt".to_string(), json!(expires_at));
    }
    document.insert(
        "current".to_string(),
        json!(current_id == Some(row.id.as_str())),
    );
    document.insert("deviceType".to_string(), json!(row.device_type));
    document.insert("deviceOS".to_string(), json!(row.device_os));
    document.insert("appVersion".to_string(), json!(row.app_version));
    document.insert(
        "isPendingSyncReset".to_string(),
        json!(row.is_pending_sync_reset != 0),
    );
    Value::Object(document)
}

fn notification_json(row: &NotificationRow) -> Value {
    let mut document = Map::new();
    document.insert("id".to_string(), json!(row.id));
    document.insert("createdAt".to_string(), json!(row.created_at));
    document.insert("level".to_string(), json!(row.level));
    document.insert("type".to_string(), json!(row.kind));
    document.insert("title".to_string(), json!(row.title));
    if let Some(description) = &row.description {
        document.insert("description".to_string(), json!(description));
    }
    if let Some(data) = &row.data {
        if let Ok(value) = serde_json::from_str::<Value>(data) {
            if !value.is_null() {
                document.insert("data".to_string(), value);
            }
        }
    }
    if let Some(read_at) = &row.read_at {
        document.insert("readAt".to_string(), json!(read_at));
    }
    Value::Object(document)
}

fn create_name(body: &Value) -> Result<String, AccountFail> {
    match body.get("name") {
        None => Ok("API Key".to_string()),
        Some(Value::String(name)) if name.is_empty() => Ok("API Key".to_string()),
        Some(Value::String(name)) => Ok(name.clone()),
        Some(_) => Err(AccountFail::BadRequest("Validation failed")),
    }
}

fn optional_string(body: &Value, key: &str) -> Result<Option<String>, AccountFail> {
    match body.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(AccountFail::BadRequest("Validation failed")),
    }
}

fn required_permissions(body: &Value) -> Result<Vec<String>, AccountFail> {
    let Some(permissions) = optional_permissions(body)? else {
        return Err(AccountFail::BadRequest("Validation failed"));
    };
    Ok(permissions)
}

fn optional_permissions(body: &Value) -> Result<Option<Vec<String>>, AccountFail> {
    let Some(value) = body.get("permissions") else {
        return Ok(None);
    };
    let Value::Array(items) = value else {
        return Err(AccountFail::BadRequest("Validation failed"));
    };
    if items.is_empty() {
        return Err(AccountFail::BadRequest("Validation failed"));
    }
    let mut permissions = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = item.as_str() else {
            return Err(AccountFail::BadRequest("Validation failed"));
        };
        if !PERMISSIONS.contains(&name) {
            return Err(AccountFail::BadRequest("Validation failed"));
        }
        permissions.push(name.to_string());
    }
    Ok(Some(permissions))
}

fn notification_ids(body: &Value) -> Result<Vec<String>, AccountFail> {
    let Some(Value::Array(items)) = body.get("ids") else {
        return Err(AccountFail::BadRequest("Validation failed"));
    };
    if items.is_empty() {
        return Err(AccountFail::BadRequest("Validation failed"));
    }
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = item.as_str() else {
            return Err(AccountFail::BadRequest("Validation failed"));
        };
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(AccountFail::BadRequest("Validation failed"));
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn required_read_at(body: &Value) -> Result<Option<String>, AccountFail> {
    let Some(value) = body.get("readAt") else {
        return Err(AccountFail::BadRequest("Validation failed"));
    };
    parse_read_at(value)
}

fn parse_read_at(value: &Value) -> Result<Option<String>, AccountFail> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => {
            let Ok(time) = DateTime::parse_from_rfc3339(text) else {
                return Err(AccountFail::BadRequest("Validation failed"));
            };
            Ok(Some(
                time.with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Millis, true),
            ))
        }
        _ => Err(AccountFail::BadRequest("Validation failed")),
    }
}

fn optional_bool(pairs: &[(String, String)], name: &str) -> Result<Option<bool>, AccountFail> {
    match pair(pairs, name).as_deref() {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(_) => Err(AccountFail::BadRequest("Validation failed")),
    }
}

fn query_pairs(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .map(|item| {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            (decode(name), decode(value))
        })
        .collect()
}

fn pair(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn decode(value: &str) -> String {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                output.push(byte);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            output.push(b' ');
        } else {
            output.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

async fn remove_profile_file(data_dir: &Path, path: &str) {
    if path.is_empty() {
        return;
    }
    let relative = Path::new(path);
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return;
    }
    let full = if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        data_dir.join(relative)
    };
    if !full.starts_with(data_dir) {
        return;
    }
    if let Err(error) = tokio::fs::remove_file(&full).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("profile image: {error}");
        }
    }
}
