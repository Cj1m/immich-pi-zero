//! A later User is created by an admin. Creating one does not add a cluster and does not send mail.

use std::collections::HashMap;
use std::path::Path;

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use serde_json::{json, Map, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};

const AVATAR_COLORS: [&str; 10] = [
    "primary", "pink", "red", "yellow", "blue", "green", "purple", "orange", "gray", "amber",
];
const ARCHIVE_SIZE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub(crate) enum UserFail {
    Db(DbError),
    BadRequest(&'static str),
    Forbidden(&'static str),
    NotFound(&'static str),
}

impl From<DbError> for UserFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for UserFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

#[derive(sqlx::FromRow)]
struct DayCount {
    day: String,
    count: i64,
}

#[derive(sqlx::FromRow)]
struct EditableUser {
    email: String,
    password: Option<String>,
    pin_code: Option<String>,
    name: String,
    avatar_color: Option<String>,
    storage_label: Option<String>,
    should_change_password: i64,
    is_admin: i64,
    quota_size_in_bytes: Option<i64>,
    quota_usage_in_bytes: i64,
}

#[derive(sqlx::FromRow)]
struct DeletionRow {
    deleted_at: Option<String>,
    profile_image_path: String,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    created_at: String,
    updated_at: String,
    device_type: String,
    device_os: String,
    app_version: Option<String>,
    is_pending_sync_reset: i64,
}

impl App {
    /// `notify` is accepted and ignored. No welcome message is sent.
    pub(crate) async fn create_user(
        &self,
        body: &Value,
    ) -> Result<crate::db::UserDocument, UserFail> {
        if !body.is_object() {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        let password = required_password(body)?;
        let email = required_email(body)?;
        let Some(name) = body.get("name").and_then(|value| value.as_str()) else {
            return Err(UserFail::BadRequest("Validation failed"));
        };
        let is_admin = optional_bool(body, "isAdmin")?.unwrap_or(false);
        let should_change = optional_bool(body, "shouldChangePassword")?.unwrap_or(true);
        let quota = quota_bytes(body)?;
        let avatar = avatar_color(body.get("avatarColor"))?;
        let taken: Option<String> = sqlx::query_scalar("SELECT id FROM user WHERE email = ?")
            .bind(&email)
            .fetch_optional(self.pool())
            .await?;
        if taken.is_some() {
            return Err(UserFail::BadRequest("Email is not available"));
        }
        let hash = bcrypt::hash(password, 10).map_err(|error| DbError::Store(error.to_string()))?;
        let id = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        let inserted = sqlx::query(
            "INSERT INTO user (
                id, email, password, pinCode, createdAt, profileImagePath, isAdmin,
                shouldChangePassword, avatarColor, deletedAt, oauthId, updatedAt,
                storageLabel, name, quotaSizeInBytes, quotaUsageInBytes, status,
                profileChangedAt, updateId, clusterGroupId
            ) VALUES (
                ?, ?, ?, NULL, ?, '', ?,
                ?, ?, NULL, NULL, ?,
                NULL, ?, ?, 0, 'active',
                ?, ?, NULL
            )",
        )
        .bind(&id)
        .bind(&email)
        .bind(hash)
        .bind(&now)
        .bind(i64::from(is_admin))
        .bind(i64::from(should_change))
        .bind(avatar)
        .bind(&now)
        .bind(name)
        .bind(quota)
        .bind(&now)
        .bind(&update_id)
        .execute(self.pool())
        .await;
        if let Err(error) = inserted {
            if is_unique(&error) {
                return Err(UserFail::BadRequest("Email is not available"));
            }
            return Err(error.into());
        }
        self.read_user(&id, true)
            .await?
            .ok_or(UserFail::BadRequest("User not found"))
    }

    /// Any subset of the admin edit fields. Omitted keys stay as they are.
    pub(crate) async fn update_admin_user(
        &self,
        actor_id: &str,
        actor_is_admin: bool,
        id: &str,
        body: &Value,
    ) -> Result<crate::db::UserDocument, UserFail> {
        if !body.is_object() {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        let current = self.editable_user(id).await?;
        let is_admin = update_bool(body, "isAdmin")?;
        if let Some(is_admin) = is_admin {
            if is_admin != actor_is_admin && actor_id == id {
                return Err(UserFail::BadRequest(
                    "Admin status can only be changed by another admin",
                ));
            }
        }
        let email = update_email(body)?;
        if let Some(email) = &email {
            let taken: Option<String> = sqlx::query_scalar(
                "SELECT id FROM user WHERE email = ? AND deletedAt IS NULL AND id != ?",
            )
            .bind(email)
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
            if taken.is_some() {
                return Err(UserFail::BadRequest("Email is not available"));
            }
        }
        let storage = update_storage_label(body)?;
        if let Some(Some(label)) = &storage {
            let taken: Option<String> = sqlx::query_scalar(
                "SELECT id FROM user WHERE storageLabel = ? AND deletedAt IS NULL AND id != ?",
            )
            .bind(label)
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
            if taken.is_some() {
                return Err(UserFail::BadRequest(
                    "Storage label already in use by another account",
                ));
            }
        }
        let password = update_password(body)?;
        let pin = update_pin(body)?;
        let name = update_name(body)?;
        let avatar = update_avatar(body)?;
        let should_change = update_bool(body, "shouldChangePassword")?;
        let quota = quota_change(body)?;
        let mut usage = current.quota_usage_in_bytes;
        if let Some(Some(bytes)) = quota {
            if bytes > 0 && Some(bytes) != current.quota_size_in_bytes {
                usage = self.recount_usage(id).await?;
            }
        }
        let updated = sqlx::query(
            "UPDATE user SET
                email = ?, password = ?, pinCode = ?, name = ?, avatarColor = ?,
                storageLabel = ?, shouldChangePassword = ?, isAdmin = ?,
                quotaSizeInBytes = ?, quotaUsageInBytes = ?, updatedAt = ?, updateId = ?
             WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(email.as_deref().unwrap_or(&current.email))
        .bind(password.unwrap_or(current.password))
        .bind(match &pin {
            Some(value) => value.as_deref(),
            None => current.pin_code.as_deref(),
        })
        .bind(name.as_deref().unwrap_or(&current.name))
        .bind(match &avatar {
            Some(value) => value.as_deref(),
            None => current.avatar_color.as_deref(),
        })
        .bind(match &storage {
            Some(value) => value.as_deref(),
            None => current.storage_label.as_deref(),
        })
        .bind(i64::from(
            should_change.unwrap_or(current.should_change_password != 0),
        ))
        .bind(i64::from(is_admin.unwrap_or(current.is_admin != 0)))
        .bind(match &quota {
            Some(value) => *value,
            None => current.quota_size_in_bytes,
        })
        .bind(usage)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(id)
        .execute(self.pool())
        .await;
        if let Err(error) = updated {
            if let Some(text) = unique_message(&error) {
                return Err(UserFail::BadRequest(text));
            }
            return Err(error.into());
        }
        self.read_user(id, false)
            .await?
            .ok_or(UserFail::BadRequest("User not found"))
    }

    pub(crate) async fn delete_admin_user(
        &self,
        actor_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<crate::db::UserDocument, UserFail> {
        if !body.is_object() {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        let force = match body.get("force") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(UserFail::BadRequest("Validation failed")),
        };
        if self.read_user(id, false).await?.is_none() {
            return Err(UserFail::BadRequest("User not found"));
        }
        if actor_id == id {
            return Err(UserFail::Forbidden("Cannot delete your own account"));
        }
        let now = now_iso();
        sqlx::query("UPDATE album SET deletedAt = ? WHERE ownerId = ? AND deletedAt IS NULL")
            .bind(&now)
            .bind(id)
            .execute(self.pool())
            .await?;
        let status = if force { "removing" } else { "deleted" };
        sqlx::query(
            "UPDATE user SET status = ?, deletedAt = ?, updatedAt = ?, updateId = ? WHERE id = ?",
        )
        .bind(status)
        .bind(&now)
        .bind(&now)
        .bind(new_update_id())
        .bind(id)
        .execute(self.pool())
        .await?;
        let user = self
            .read_user(id, true)
            .await?
            .ok_or(UserFail::BadRequest("User not found"))?;
        if force {
            if let Err(error) = self.hard_delete_user(id, true).await {
                eprintln!("user delete: {error:?}");
            }
        }
        Ok(user)
    }

    pub(crate) async fn restore_admin_user(
        &self,
        id: &str,
    ) -> Result<crate::db::UserDocument, UserFail> {
        if self.read_user(id, true).await?.is_none() {
            return Err(UserFail::BadRequest("User not found"));
        }
        sqlx::query("UPDATE album SET deletedAt = NULL WHERE ownerId = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        let now = now_iso();
        sqlx::query(
            "UPDATE user SET status = 'active', deletedAt = NULL, updatedAt = ?, updateId = ?
             WHERE id = ?",
        )
        .bind(&now)
        .bind(new_update_id())
        .bind(id)
        .execute(self.pool())
        .await?;
        self.read_user(id, true)
            .await?
            .ok_or(UserFail::BadRequest("User not found"))
    }

    /// Remove a User who is old enough, or any User when `force` is set.
    /// Cascaded assets do not write an asset-delete sync line.
    pub async fn hard_delete_user(&self, id: &str, force: bool) -> Result<(), DbError> {
        let row = self.deletion_row(id).await?;
        let Some(row) = row else {
            return Ok(());
        };
        if !force && !self.ready_for_deletion(row.deleted_at.as_deref()).await? {
            return Ok(());
        }
        self.remove_user_files(id, row.profile_image_path.as_str())
            .await;
        self.purge_owned_albums(id).await?;
        sqlx::query("INSERT INTO user_audit (id, userId, updateId) VALUES (?, ?, ?)")
            .bind(new_id())
            .bind(id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        sqlx::query("DELETE FROM user WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        let packet = format!("42{}", json!(["on_user_delete", id]));
        self.broadcast_event(&packet);
        Ok(())
    }

    /// Nightly cleanup. Skips a User who is younger than `user.deleteDelay`.
    #[doc(hidden)]
    pub async fn purge_expired_users(&self) -> Result<(), DbError> {
        let cutoff = self.deletion_cutoff().await?;
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE deletedAt IS NOT NULL AND deletedAt < ?")
                .bind(&cutoff)
                .fetch_all(self.pool())
                .await?;
        for id in ids {
            self.hard_delete_user(&id, false).await?;
        }
        Ok(())
    }

    pub(crate) async fn set_my_password(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<crate::db::UserDocument, UserFail> {
        if !body.is_object() {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        // Name and email on this call wait with the rest of the user update screen.
        if let Some(password) = body.get("password") {
            match password {
                Value::String(password) if !password.is_empty() => {
                    self.store_password(user_id, password).await?;
                }
                Value::String(_) | Value::Null => {}
                _ => return Err(UserFail::BadRequest("Validation failed")),
            }
        }
        self.read_user(user_id, true)
            .await?
            .ok_or(UserFail::BadRequest("User not found"))
    }

    pub(crate) async fn change_password(
        &self,
        user_id: &str,
        session_id: &str,
        body: &Value,
    ) -> Result<crate::db::UserDocument, UserFail> {
        if !body.is_object() {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        let Some(current) = body.get("password").and_then(|value| value.as_str()) else {
            return Err(UserFail::BadRequest("Validation failed"));
        };
        let Some(new_password) = body.get("newPassword").and_then(|value| value.as_str()) else {
            return Err(UserFail::BadRequest("Validation failed"));
        };
        if new_password.len() < 8 {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        if let Some(flag) = body.get("invalidateSessions") {
            if !flag.is_boolean() {
                return Err(UserFail::BadRequest("Validation failed"));
            }
        }
        let hash = self.user_password_hash(user_id).await?;
        let matches = hash
            .as_deref()
            .is_some_and(|stored| bcrypt::verify(current, stored).unwrap_or(false));
        if !matches {
            return Err(UserFail::BadRequest("Wrong password"));
        }
        self.store_password(user_id, new_password).await?;
        // v3.3.6 deletes the other sessions even when invalidateSessions is false or omitted.
        sqlx::query("DELETE FROM session WHERE userId = ? AND id != ?")
            .bind(user_id)
            .bind(session_id)
            .execute(self.pool())
            .await?;
        self.read_user(user_id, true)
            .await?
            .ok_or(UserFail::BadRequest("User not found"))
    }

    pub(crate) async fn preferences_for(&self, user_id: &str) -> Result<Value, DbError> {
        let mut document = default_preferences();
        if let Some(partial) = self.metadata_value(user_id, "preferences").await? {
            merge_known(&mut document, &partial);
        }
        Ok(document)
    }

    pub(crate) async fn update_preferences(
        &self,
        user_id: &str,
        patch: &Value,
    ) -> Result<Value, UserFail> {
        if !patch.is_object() {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        let defaults = default_preferences();
        let mut document = defaults.clone();
        if let Some(partial) = self.metadata_value(user_id, "preferences").await? {
            merge_known(&mut document, &partial);
        }
        merge_known(&mut document, patch);
        let partial = preference_partial(&defaults, &document);
        self.write_metadata(user_id, "preferences", &partial)
            .await?;
        Ok(document)
    }

    pub(crate) async fn set_onboarding(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<Value, UserFail> {
        let Some(flag) = body.get("isOnboarded").and_then(|value| value.as_bool()) else {
            return Err(UserFail::BadRequest("Validation failed"));
        };
        self.write_metadata(user_id, "onboarding", &json!({ "isOnboarded": flag }))
            .await?;
        Ok(json!({ "isOnboarded": flag }))
    }

    pub(crate) async fn set_admin_onboarding(&self, body: &Value) -> Result<(), UserFail> {
        let Some(flag) = body.get("isOnboarded").and_then(|value| value.as_bool()) else {
            return Err(UserFail::BadRequest("Validation failed"));
        };
        let text = serde_json::to_string(&json!({ "isOnboarded": flag }))
            .map_err(|error| DbError::Store(error.to_string()))?;
        sqlx::query(
            "INSERT INTO system_metadata (key, value) VALUES ('admin-onboarding', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(text)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub(crate) async fn user_statistics(&self, id: &str) -> Result<Value, UserFail> {
        let row: Option<(i64, i64, i64)> = sqlx::query_as(
            "SELECT
                COALESCE(SUM(CASE WHEN type = 'IMAGE' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'VIDEO' THEN 1 ELSE 0 END), 0),
                COUNT(*)
             FROM asset
             WHERE ownerId = ?
               AND deletedAt IS NULL
               AND visibility IN ('timeline', 'archive')",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        let (images, videos, total) = row.unwrap_or((0, 0, 0));
        Ok(json!({ "images": images, "videos": videos, "total": total }))
    }

    pub(crate) async fn user_sessions(&self, id: &str) -> Result<Value, UserFail> {
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT session.id AS id,
                    session.createdAt AS created_at,
                    session.updatedAt AS updated_at,
                    session.deviceType AS device_type,
                    session.deviceOS AS device_os,
                    session.appVersion AS app_version,
                    session.isPendingSyncReset AS is_pending_sync_reset
             FROM session
             JOIN user ON user.id = session.userId AND user.deletedAt IS NULL
             WHERE session.userId = ?
             ORDER BY session.updatedAt DESC, session.createdAt DESC",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        let sessions = rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "createdAt": row.created_at,
                    "updatedAt": row.updated_at,
                    "deviceType": row.device_type,
                    "deviceOS": row.device_os,
                    "appVersion": row.app_version,
                    "current": false,
                    "isPendingSyncReset": row.is_pending_sync_reset != 0,
                })
            })
            .collect::<Vec<_>>();
        Ok(Value::Array(sessions))
    }

    pub(crate) async fn user_heatmap(
        &self,
        id: &str,
        from: Option<&str>,
        to: Option<&str>,
        kind: &str,
    ) -> Result<Value, UserFail> {
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE id = ? AND deletedAt IS NULL")
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        if exists.is_none() {
            return Err(UserFail::BadRequest("User not found"));
        }
        let column = match kind {
            "Upload" => "createdAt",
            "Taken" => "localDateTime",
            _ => return Err(UserFail::BadRequest("Validation failed")),
        };
        let today = Utc::now().date_naive();
        let to_day = match to {
            Some(value) => parse_day(value)?,
            None => today,
        };
        let from_day = match from {
            Some(value) => parse_day(value)?,
            None => to_day
                .checked_sub_signed(Duration::weeks(52))
                .and_then(|day| day.checked_add_signed(Duration::days(1)))
                .unwrap_or(to_day),
        };
        if from_day > to_day {
            return Err(UserFail::BadRequest("Validation failed"));
        }
        let start = format!("{from_day}T00:00:00.000Z");
        let end_day = to_day
            .checked_add_signed(Duration::days(1))
            .ok_or(UserFail::BadRequest("Validation failed"))?;
        let end = format!("{end_day}T00:00:00.000Z");
        let sql = format!(
            "SELECT substr({column}, 1, 10) AS day, COUNT(*) AS count
             FROM asset
             WHERE ownerId = ? AND deletedAt IS NULL AND {column} >= ? AND {column} < ?
             GROUP BY substr({column}, 1, 10)"
        );
        let counts = sqlx::query_as::<_, DayCount>(&sql)
            .bind(id)
            .bind(&start)
            .bind(&end)
            .fetch_all(self.pool())
            .await?;
        let counts: HashMap<String, i64> =
            counts.into_iter().map(|row| (row.day, row.count)).collect();
        let mut series = Vec::new();
        let mut total = 0_i64;
        let mut day = from_day;
        loop {
            let key = day.format("%Y-%m-%d").to_string();
            let count = counts.get(&key).copied().unwrap_or(0);
            total += count;
            series.push(json!({ "date": key, "count": count }));
            if day == to_day {
                break;
            }
            day = day
                .checked_add_signed(Duration::days(1))
                .ok_or(UserFail::BadRequest("Validation failed"))?;
        }
        Ok(json!({
            "from": from_day.format("%Y-%m-%d").to_string(),
            "to": to_day.format("%Y-%m-%d").to_string(),
            "series": series,
            "totalCount": total,
        }))
    }

    pub(crate) async fn profile_image(&self, id: &str) -> Result<(String, Vec<u8>), UserFail> {
        let path: Option<String> = sqlx::query_scalar(
            "SELECT profileImagePath FROM user WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            return Err(UserFail::NotFound("Not found"));
        };
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| UserFail::NotFound("Not found"))?;
        Ok((image_type(&path).to_string(), bytes))
    }

    /// Byte counts of the filesystem that holds `{data}/thumbs`.
    /// The text fields are v3.3.6 `asHumanReadable` labels. The app prints those strings.
    pub(crate) fn storage_report(&self) -> Value {
        let thumbs = self.data_dir().join("thumbs");
        let _ = std::fs::create_dir_all(&thumbs);
        let path = if thumbs.is_dir() {
            thumbs
        } else {
            self.data_dir().to_path_buf()
        };
        let (total, used, available) = disk_bytes(&path).unwrap_or((0, 0, 0));
        let percentage = if total == 0 {
            0.0
        } else {
            let text = format!("{:.2}", (used as f64 / total as f64) * 100.0);
            text.parse::<f64>().unwrap_or(0.0)
        };
        json!({
            "diskSize": human_readable_bytes(total),
            "diskUse": human_readable_bytes(used),
            "diskAvailable": human_readable_bytes(available),
            "diskSizeRaw": total,
            "diskUseRaw": used,
            "diskAvailableRaw": available,
            "diskUsagePercentage": percentage,
        })
    }

    async fn store_password(&self, user_id: &str, password: &str) -> Result<(), UserFail> {
        let hash = bcrypt::hash(password, 10).map_err(|error| DbError::Store(error.to_string()))?;
        sqlx::query(
            "UPDATE user
             SET password = ?, shouldChangePassword = 0, updatedAt = ?, updateId = ?
             WHERE id = ?",
        )
        .bind(hash)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(user_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn metadata_value(&self, user_id: &str, key: &str) -> Result<Option<Value>, DbError> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM user_metadata WHERE userId = ? AND key = ?")
                .bind(user_id)
                .bind(key)
                .fetch_optional(self.pool())
                .await?;
        let Some(value) = value else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&value).ok())
    }

    async fn write_metadata(&self, user_id: &str, key: &str, value: &Value) -> Result<(), DbError> {
        let text =
            serde_json::to_string(value).map_err(|error| DbError::Store(error.to_string()))?;
        sqlx::query(
            "INSERT INTO user_metadata (userId, key, value, updateId, updatedAt)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(userId, key) DO UPDATE SET
                value = excluded.value,
                updateId = excluded.updateId,
                updatedAt = excluded.updatedAt",
        )
        .bind(user_id)
        .bind(key)
        .bind(text)
        .bind(new_update_id())
        .bind(now_iso())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn editable_user(&self, id: &str) -> Result<EditableUser, UserFail> {
        let row = sqlx::query_as::<_, EditableUser>(
            "SELECT email, password, pinCode AS pin_code, name, avatarColor AS avatar_color,
                    storageLabel AS storage_label, shouldChangePassword AS should_change_password,
                    isAdmin AS is_admin, quotaSizeInBytes AS quota_size_in_bytes,
                    quotaUsageInBytes AS quota_usage_in_bytes
             FROM user WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        row.ok_or(UserFail::BadRequest("User not found"))
    }

    async fn recount_usage(&self, id: &str) -> Result<i64, UserFail> {
        let usage: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(asset_exif.fileSizeInByte), 0)
             FROM asset
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.libraryId IS NULL AND asset.ownerId = ?",
        )
        .bind(id)
        .fetch_one(self.pool())
        .await?;
        Ok(usage)
    }

    async fn deletion_row(&self, id: &str) -> Result<Option<DeletionRow>, DbError> {
        Ok(sqlx::query_as::<_, DeletionRow>(
            "SELECT deletedAt AS deleted_at, profileImagePath AS profile_image_path
             FROM user WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    async fn ready_for_deletion(&self, deleted_at: Option<&str>) -> Result<bool, DbError> {
        let Some(deleted_at) = deleted_at else {
            return Ok(false);
        };
        Ok(deleted_at < self.deletion_cutoff().await?.as_str())
    }

    async fn deletion_cutoff(&self) -> Result<String, DbError> {
        let config = self.admin_config().await?;
        let days = config["user"]["deleteDelay"].as_i64().unwrap_or(7);
        let days = if days < 1 { 7 } else { days };
        let cutoff = Utc::now() - Duration::days(days);
        Ok(cutoff.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    }

    async fn remove_user_files(&self, id: &str, profile_path: &str) {
        let assets: Vec<(String, String)> =
            match sqlx::query_as("SELECT id, originalPath FROM asset WHERE ownerId = ?")
                .bind(id)
                .fetch_all(self.pool())
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    eprintln!("user delete files: {error}");
                    Vec::new()
                }
            };
        for (asset_id, original_path) in &assets {
            if let Err(error) = self.store().delete(original_path).await {
                eprintln!("user delete object: {error}");
            }
            let sidecar = format!("{original_path}.xmp");
            if let Err(error) = self.store().delete(&sidecar).await {
                eprintln!("user delete sidecar: {error}");
            }
            crate::thumbs::remove_thumbs(self.data_dir(), asset_id);
        }
        let files: Vec<String> = sqlx::query_scalar(
            "SELECT asset_file.path FROM asset_file
             JOIN asset ON asset.id = asset_file.assetId
             WHERE asset.ownerId = ?",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await
        .unwrap_or_default();
        for path in files {
            remove_local_file(self.data_dir(), &path).await;
        }
        remove_local_file(self.data_dir(), profile_path).await;
        let people = self.data_dir().join("thumbs").join(id);
        if let Err(error) = tokio::fs::remove_dir_all(&people).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("user delete people: {error}");
            }
        }
    }
}

pub(crate) fn default_preferences() -> Value {
    json!({
        "albums": { "defaultAssetOrder": "desc" },
        "folders": { "enabled": false, "sidebarWeb": false },
        "memories": { "enabled": true, "duration": 5, "sidebarWeb": false },
        "people": {
            "enabled": true,
            "sidebarWeb": false,
            "minimumFaces": 3,
            "updateStrategy": "everyone",
        },
        "sharedLinks": { "enabled": true, "sidebarWeb": false },
        "ratings": { "enabled": false },
        "tags": { "enabled": false, "sidebarWeb": false },
        "emailNotifications": { "enabled": true, "albumInvite": true, "albumUpdate": true },
        "download": {
            "archiveSize": ARCHIVE_SIZE_BYTES,
            "includeEmbeddedVideos": false,
        },
        "purchase": {
            "showSupportBadge": true,
            "hideBuyButtonUntil": local_date_iso(2022, 2, 12),
        },
        "cast": { "gCastEnabled": false },
        "recentlyAdded": { "sidebarWeb": false },
    })
}

fn local_date_iso(year: i32, month: u32, day: u32) -> String {
    chrono::Local
        .with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("local date")
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn merge_known(base: &mut Value, patch: &Value) {
    let (Value::Object(base_map), Value::Object(patch_map)) = (base, patch) else {
        return;
    };
    for (key, value) in patch_map {
        let Some(existing) = base_map.get_mut(key) else {
            continue;
        };
        if existing.is_object() && value.is_object() {
            merge_known(existing, value);
        } else if !existing.is_object() {
            *existing = value.clone();
        }
    }
}

fn preference_partial(defaults: &Value, current: &Value) -> Value {
    match (defaults, current) {
        (Value::Object(defaults), Value::Object(current)) => {
            let mut partial = Map::new();
            for (key, value) in current {
                match defaults.get(key) {
                    Some(default) if default == value => {}
                    Some(default) if default.is_object() && value.is_object() => {
                        let child = preference_partial(default, value);
                        if child.as_object().is_some_and(|object| !object.is_empty()) {
                            partial.insert(key.clone(), child);
                        }
                    }
                    _ => {
                        partial.insert(key.clone(), value.clone());
                    }
                }
            }
            Value::Object(partial)
        }
        _ => current.clone(),
    }
}

fn required_password(body: &Value) -> Result<&str, UserFail> {
    match body.get("password") {
        None | Some(Value::Null) => Err(UserFail::BadRequest("password is required")),
        Some(Value::String(password)) if password.is_empty() => {
            Err(UserFail::BadRequest("password is required"))
        }
        Some(Value::String(password)) => Ok(password),
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn required_email(body: &Value) -> Result<String, UserFail> {
    let Some(email) = body.get("email").and_then(|value| value.as_str()) else {
        return Err(UserFail::BadRequest("Validation failed"));
    };
    let email = email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(UserFail::BadRequest("Validation failed"));
    }
    Ok(email)
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !email.contains(char::is_whitespace)
        && !domain.contains('@')
}

fn optional_bool(body: &Value, key: &str) -> Result<Option<bool>, UserFail> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn quota_bytes(body: &Value) -> Result<Option<i64>, UserFail> {
    match body.get("quotaSizeInBytes") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let Some(value) = number.as_i64() else {
                return Err(UserFail::BadRequest("Validation failed"));
            };
            if value < 0 {
                return Err(UserFail::BadRequest("Validation failed"));
            }
            Ok(Some(value))
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn avatar_color(value: Option<&Value>) -> Result<Option<String>, UserFail> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(color)) if AVATAR_COLORS.contains(&color.as_str()) => {
            Ok(Some(color.clone()))
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn parse_day(value: &str) -> Result<NaiveDate, UserFail> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| UserFail::BadRequest("Validation failed"))
}

fn image_type(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or("");
    if extension.eq_ignore_ascii_case("png") {
        "image/png"
    } else if extension.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else if extension.eq_ignore_ascii_case("gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

fn update_bool(body: &Value, key: &str) -> Result<Option<bool>, UserFail> {
    match body.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn update_email(body: &Value) -> Result<Option<String>, UserFail> {
    match body.get("email") {
        None => Ok(None),
        Some(Value::String(email)) => {
            let email = email.trim().to_lowercase();
            if !valid_email(&email) {
                return Err(UserFail::BadRequest("Validation failed"));
            }
            Ok(Some(email))
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn update_name(body: &Value) -> Result<Option<String>, UserFail> {
    match body.get("name") {
        None => Ok(None),
        Some(Value::String(name)) => Ok(Some(name.clone())),
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn update_avatar(body: &Value) -> Result<Option<Option<String>>, UserFail> {
    if body.get("avatarColor").is_none() {
        return Ok(None);
    }
    Ok(Some(avatar_color(body.get("avatarColor"))?))
}

fn update_password(body: &Value) -> Result<Option<Option<String>>, UserFail> {
    match body.get("password") {
        None => Ok(None),
        Some(Value::String(password)) if password.is_empty() => Ok(Some(Some(String::new()))),
        Some(Value::String(password)) => {
            let hash =
                bcrypt::hash(password, 10).map_err(|error| DbError::Store(error.to_string()))?;
            Ok(Some(Some(hash)))
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn update_pin(body: &Value) -> Result<Option<Option<String>>, UserFail> {
    match body.get("pinCode") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(pin)) if pin.len() == 6 && pin.chars().all(|ch| ch.is_ascii_digit()) => {
            let hash = bcrypt::hash(pin, 10).map_err(|error| DbError::Store(error.to_string()))?;
            Ok(Some(Some(hash)))
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn update_storage_label(body: &Value) -> Result<Option<Option<String>>, UserFail> {
    match body.get("storageLabel") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(label)) => {
            let label = sanitize_filename(label);
            if label.is_empty() {
                Ok(Some(None))
            } else {
                Ok(Some(Some(label)))
            }
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn quota_change(body: &Value) -> Result<Option<Option<i64>>, UserFail> {
    match body.get("quotaSizeInBytes") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::Number(number)) => {
            let Some(value) = number.as_i64() else {
                return Err(UserFail::BadRequest("Validation failed"));
            };
            if value < 0 {
                return Err(UserFail::BadRequest("Validation failed"));
            }
            Ok(Some(Some(value)))
        }
        Some(_) => Err(UserFail::BadRequest("Validation failed")),
    }
}

fn unique_message(error: &sqlx::Error) -> Option<&'static str> {
    if !is_unique(error) {
        return None;
    }
    let sqlx::Error::Database(database) = error else {
        return None;
    };
    if database.message().contains("storageLabel") {
        Some("Storage label already in use by another account")
    } else {
        Some("Email is not available")
    }
}

/// `sanitize-filename`: drop reserved characters, Windows device names, and trailing dots.
pub(crate) fn sanitize_filename(input: &str) -> String {
    let mut out = String::new();
    for character in input.chars() {
        let code = character as u32;
        let illegal = matches!(
            character,
            '/' | '?' | '<' | '>' | '\\' | ':' | '*' | '|' | '"'
        ) || code <= 0x1f
            || (0x80..=0x9f).contains(&code);
        if !illegal {
            out.push(character);
        }
    }
    if out.chars().all(|character| character == '.') {
        return String::new();
    }
    let lower = out.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or("");
    let reserved = matches!(stem, "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes().get(3).is_some_and(u8::is_ascii_digit));
    if reserved {
        return String::new();
    }
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    if out.len() > 255 {
        let mut end = 255;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

async fn remove_local_file(data_dir: &Path, path: &str) {
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
            eprintln!("user delete file: {error}");
        }
    }
}

fn is_unique(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database) = error else {
        return false;
    };
    database
        .code()
        .is_some_and(|code| code == "2067" || code == "1555")
        || database.message().contains("UNIQUE constraint failed")
}

fn disk_bytes(path: &Path) -> Option<(u64, u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // The libc layout is the one this operating system uses for the thumbnail filesystem.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    let frsize = width_u64(stats.f_frsize);
    let total = width_u64(stats.f_blocks).saturating_mul(frsize);
    let free = width_u64(stats.f_bfree).saturating_mul(frsize);
    let available = width_u64(stats.f_bavail).saturating_mul(frsize);
    Some((total, total.saturating_sub(free), available))
}

fn width_u64(value: impl TryInto<u64>) -> u64 {
    value.try_into().unwrap_or(0)
}

/// Same labels as v3.3.6 `asHumanReadable`: 1024-based, one decimal past bytes.
fn human_readable_bytes(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut magnitude = 0usize;
    let mut remainder = bytes as f64;
    while remainder >= 1024.0 {
        if magnitude + 1 < UNITS.len() {
            magnitude += 1;
            remainder /= 1024.0;
        } else {
            break;
        }
    }
    if magnitude == 0 {
        format!("{remainder:.0} {}", UNITS[magnitude])
    } else {
        format!("{remainder:.1} {}", UNITS[magnitude])
    }
}

#[cfg(test)]
mod tests {
    use super::human_readable_bytes;

    #[test]
    fn storage_labels_match_v3_3_6() {
        assert_eq!(human_readable_bytes(0), "0 B");
        assert_eq!(human_readable_bytes(500), "500 B");
        assert_eq!(human_readable_bytes(300), "300 B");
        assert_eq!(human_readable_bytes(500_000), "488.3 KiB");
        assert_eq!(human_readable_bytes(300_000), "293.0 KiB");
        assert_eq!(human_readable_bytes(500_000_000), "476.8 MiB");
        assert_eq!(human_readable_bytes(300_000_000), "286.1 MiB");
        assert_eq!(human_readable_bytes(500_000_000_000), "465.7 GiB");
        assert_eq!(human_readable_bytes(300_000_000_000), "279.4 GiB");
        assert_eq!(human_readable_bytes(500_000_000_000_000), "454.7 TiB");
        assert_eq!(human_readable_bytes(300_000_000_000_000), "272.8 TiB");
        assert_eq!(human_readable_bytes(500_000_000_000_000_000), "444.1 PiB");
        assert_eq!(human_readable_bytes(300_000_000_000_000_000), "266.5 PiB");
    }
}
