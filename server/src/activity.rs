//! Album likes and comments. They stay off the sync stream.

use http::StatusCode;
use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};

pub(crate) enum ActivityError {
    Db(DbError),
    Status(StatusCode, &'static str),
}

impl From<DbError> for ActivityError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for ActivityError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

pub(crate) enum ActivityWrite {
    Created(Value),
    Existing(Value),
}

pub(crate) struct ActivitySearch {
    pub album_id: String,
    pub user_id: Option<String>,
    pub asset_id: Option<String>,
    pub album_level: bool,
    pub liked: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct ActivityRow {
    id: String,
    created_at: String,
    asset_id: Option<String>,
    comment: Option<String>,
    is_liked: i64,
    user_id: String,
    user_name: String,
    email: String,
    avatar_color: Option<String>,
    profile_image_path: String,
    profile_changed_at: String,
}

const ACTIVITY_SQL: &str = "
SELECT activity.id AS id, activity.createdAt AS created_at, activity.assetId AS asset_id,
       activity.comment AS comment, activity.isLiked AS is_liked,
       user.id AS user_id, user.name AS user_name, user.email AS email,
       user.avatarColor AS avatar_color, user.profileImagePath AS profile_image_path,
       user.profileChangedAt AS profile_changed_at
FROM activity
INNER JOIN user ON user.id = activity.userId AND user.deletedAt IS NULL
LEFT JOIN asset ON asset.id = activity.assetId
";

impl App {
    pub(crate) async fn list_activities(
        &self,
        user_id: &str,
        search: &ActivitySearch,
    ) -> Result<Vec<Value>, ActivityError> {
        self.require_album_read(user_id, &search.album_id).await?;
        let rows: Vec<ActivityRow> = sqlx::query_as(&format!(
            "{ACTIVITY_SQL}
             WHERE activity.albumId = ?
               AND (activity.assetId IS NULL OR asset.deletedAt IS NULL)
               AND (? IS NULL OR activity.userId = ?)
               AND (? = 0 OR activity.assetId IS NULL)
               AND (? IS NULL OR activity.assetId = ?)
               AND (? IS NULL OR activity.isLiked = ?)
             ORDER BY activity.createdAt ASC"
        ))
        .bind(&search.album_id)
        .bind(&search.user_id)
        .bind(&search.user_id)
        .bind(i64::from(search.album_level))
        .bind(&search.asset_id)
        .bind(&search.asset_id)
        .bind(search.liked)
        .bind(search.liked)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(activity_json).collect())
    }

    pub(crate) async fn activity_statistics(
        &self,
        user_id: &str,
        album_id: &str,
        asset_id: Option<&str>,
    ) -> Result<Value, ActivityError> {
        self.require_album_read(user_id, album_id).await?;
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT
                COALESCE(SUM(CASE WHEN activity.isLiked = 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN activity.isLiked = 1 THEN 1 ELSE 0 END), 0)
             FROM activity
             INNER JOIN user ON user.id = activity.userId AND user.deletedAt IS NULL
             LEFT JOIN asset ON asset.id = activity.assetId
             WHERE activity.albumId = ?
               AND (
                    asset.id IS NULL
                    OR (asset.deletedAt IS NULL AND asset.visibility != 'locked')
               )
               AND (? IS NULL OR activity.assetId = ?)",
        )
        .bind(album_id)
        .bind(asset_id)
        .bind(asset_id)
        .fetch_one(self.pool())
        .await?;
        Ok(json!({ "comments": counts.0, "likes": counts.1 }))
    }

    pub(crate) async fn create_activity(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<ActivityWrite, ActivityError> {
        let album_id = required_id(body, "albumId")?;
        let asset_id = optional_id(body, "assetId")?;
        let kind = body.get("type").and_then(Value::as_str).unwrap_or("");
        let liked = match kind {
            "like" => true,
            "comment" => false,
            _ => {
                return Err(ActivityError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
            }
        };
        let comment = match body.get("comment") {
            None => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => {
                return Err(ActivityError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
            }
        };
        if liked {
            if comment.as_ref().is_some_and(|text| !text.is_empty()) {
                return Err(ActivityError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
            }
        } else if comment.as_ref().is_none_or(|text| text.trim().is_empty()) {
            return Err(ActivityError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
        }
        if !self.activity_enabled(user_id, &album_id).await? {
            return Err(ActivityError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no activity.create access",
            ));
        }
        if liked {
            if let Some(existing) = self.existing_like(user_id, &album_id, asset_id.as_deref()).await? {
                return Ok(ActivityWrite::Existing(activity_json(&existing)));
            }
        }
        let id = new_id();
        let now = now_iso();
        let stored_comment = if liked { None } else { comment };
        sqlx::query(
            "INSERT INTO activity
                (id, createdAt, updatedAt, albumId, userId, assetId, comment, isLiked, updateId)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&now)
        .bind(&now)
        .bind(&album_id)
        .bind(user_id)
        .bind(&asset_id)
        .bind(&stored_comment)
        .bind(i64::from(liked))
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        let row = self.activity_row(&id).await?;
        Ok(ActivityWrite::Created(activity_json(&row)))
    }

    pub(crate) async fn delete_activity(&self, user_id: &str, id: &str) -> Result<(), ActivityError> {
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM activity
             WHERE id = ?
               AND (
                    userId = ?
                    OR albumId IN (SELECT id FROM album WHERE ownerId = ?)
               )",
        )
        .bind(id)
        .bind(user_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        if allowed.is_none() {
            return Err(ActivityError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no activity.delete access",
            ));
        }
        sqlx::query("DELETE FROM activity WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn require_album_read(&self, user_id: &str, album_id: &str) -> Result<(), ActivityError> {
        let member: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM album_user WHERE albumId = ? AND userId = ?",
        )
        .bind(album_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        if member.is_none() {
            return Err(ActivityError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no album.read access",
            ));
        }
        Ok(())
    }

    async fn activity_enabled(&self, user_id: &str, album_id: &str) -> Result<bool, ActivityError> {
        let flag: Option<i64> = sqlx::query_scalar(
            "SELECT album.isActivityEnabled
             FROM album
             JOIN album_user ON album_user.albumId = album.id
             WHERE album.id = ? AND album_user.userId = ?",
        )
        .bind(album_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(flag == Some(1))
    }

    async fn existing_like(
        &self,
        user_id: &str,
        album_id: &str,
        asset_id: Option<&str>,
    ) -> Result<Option<ActivityRow>, ActivityError> {
        let row: Option<ActivityRow> = sqlx::query_as(&format!(
            "{ACTIVITY_SQL}
             WHERE activity.userId = ? AND activity.albumId = ? AND activity.isLiked = 1
               AND ((? IS NULL AND activity.assetId IS NULL) OR activity.assetId = ?)
             LIMIT 1"
        ))
        .bind(user_id)
        .bind(album_id)
        .bind(asset_id)
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    async fn activity_row(&self, id: &str) -> Result<ActivityRow, ActivityError> {
        let row: Option<ActivityRow> = sqlx::query_as(&format!("{ACTIVITY_SQL} WHERE activity.id = ?"))
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        row.ok_or_else(|| ActivityError::Status(StatusCode::BAD_REQUEST, "Not found or no activity.create access"))
    }
}

fn activity_json(row: &ActivityRow) -> Value {
    json!({
        "id": row.id,
        "createdAt": row.created_at,
        "assetId": row.asset_id,
        "comment": row.comment,
        "type": if row.is_liked == 0 { "comment" } else { "like" },
        "user": {
            "id": row.user_id,
            "name": row.user_name,
            "email": row.email,
            "avatarColor": row.avatar_color,
            "profileImagePath": row.profile_image_path,
            "profileChangedAt": row.profile_changed_at,
        },
    })
}

fn required_id(body: &Value, key: &str) -> Result<String, ActivityError> {
    match body.get(key).and_then(Value::as_str) {
        Some(id) if uuid::Uuid::parse_str(id).is_ok() => Ok(id.to_string()),
        _ => Err(ActivityError::Status(StatusCode::BAD_REQUEST, "Validation failed")),
    }
}

fn optional_id(body: &Value, key: &str) -> Result<Option<String>, ActivityError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if uuid::Uuid::parse_str(id).is_ok() => Ok(Some(id.clone())),
        Some(_) => Err(ActivityError::Status(StatusCode::BAD_REQUEST, "Validation failed")),
    }
}
