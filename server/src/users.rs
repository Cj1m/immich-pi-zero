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
    /// The three text fields are those same counts, not a unit label.
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
            "diskSize": total.to_string(),
            "diskUse": used.to_string(),
            "diskAvailable": available.to_string(),
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
