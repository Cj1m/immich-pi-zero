//! Cluster membership for sharing people.
//! Creating a user does not create a group. Leave is the only insert, and only when another member remains.

use serde_json::{json, Value};

use crate::db::{avatar_color_for_email, new_id, new_update_id, now_iso, App};

const READ_MISS: &str = "Not found or no clusterGroup.read access";
const CREATE_MISS: &str = "Not found or no clusterGroupRequest.create access";
const REQUEST_READ_MISS: &str = "Not found or no clusterGroupRequest.read access";
const DELETE_MISS: &str = "Not found or no clusterGroupRequest.delete access";
const LEAVE_MISS: &str = "Not found or no clusterGroup.leave access";

pub(crate) enum ClusterFail {
    Db(crate::db::DbError),
    BadRequest(&'static str),
}

impl From<crate::db::DbError> for ClusterFail {
    fn from(error: crate::db::DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for ClusterFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

pub(crate) struct ClusterRequest {
    pub(crate) created: bool,
    pub(crate) document: Value,
}

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: String,
    cluster_group_id: String,
    user_id: String,
    created_at: String,
}

#[derive(sqlx::FromRow)]
struct PublicUser {
    id: String,
    name: String,
    email: String,
    profile_image_path: String,
    avatar_color: Option<String>,
    profile_changed_at: String,
}

impl App {
    pub(crate) async fn list_my_cluster_requests(
        &self,
        user_id: &str,
    ) -> Result<Value, ClusterFail> {
        let rows = request_rows(
            self,
            "SELECT id, clusterGroupId AS cluster_group_id, userId AS user_id, createdAt AS created_at
             FROM cluster_group_request WHERE userId = ? ORDER BY createdAt ASC",
            user_id,
        )
        .await?;
        Ok(Value::Array(rows.iter().map(request_json).collect()))
    }

    /// Members only. An invited user can read the group and its users, not its outgoing requests.
    pub(crate) async fn list_group_requests(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> Result<Value, ClusterFail> {
        if !self.is_member(user_id, group_id).await? {
            return Err(ClusterFail::BadRequest(READ_MISS));
        }
        let rows = request_rows(
            self,
            "SELECT id, clusterGroupId AS cluster_group_id, userId AS user_id, createdAt AS created_at
             FROM cluster_group_request WHERE clusterGroupId = ? ORDER BY createdAt ASC",
            group_id,
        )
        .await?;
        Ok(Value::Array(rows.iter().map(request_json).collect()))
    }

    pub(crate) async fn list_group_users(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> Result<Value, ClusterFail> {
        if !self.can_read_group(user_id, group_id).await? {
            return Err(ClusterFail::BadRequest(READ_MISS));
        }
        let rows: Vec<PublicUser> = sqlx::query_as(
            "SELECT id, name, email, profileImagePath AS profile_image_path,
                    avatarColor AS avatar_color, profileChangedAt AS profile_changed_at
             FROM user
             WHERE clusterGroupId = ? AND deletedAt IS NULL
             ORDER BY CASE WHEN id = ? THEN 0 ELSE 1 END, name ASC",
        )
        .bind(group_id)
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(rows.iter().map(public_user_json).collect()))
    }

    pub(crate) async fn create_cluster_request(
        &self,
        user_id: &str,
        group_id: &str,
        body: &Value,
    ) -> Result<ClusterRequest, ClusterFail> {
        if !self.is_member(user_id, group_id).await? {
            return Err(ClusterFail::BadRequest(CREATE_MISS));
        }
        let target_id = match body.get("userId") {
            Some(Value::String(value)) if is_uuid_v4(value) => value.as_str(),
            _ => return Err(ClusterFail::BadRequest("Validation failed")),
        };
        if target_id.eq_ignore_ascii_case(user_id) {
            return Err(ClusterFail::BadRequest(
                "Cannot request to join your own cluster group",
            ));
        }
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE id = ? AND deletedAt IS NULL")
                .bind(target_id)
                .fetch_optional(self.pool())
                .await?;
        if exists.is_none() {
            return Err(ClusterFail::BadRequest("User not found"));
        }
        if let Some(row) = self.request_for_pair(group_id, target_id).await? {
            return Ok(ClusterRequest {
                created: false,
                document: request_json(&row),
            });
        }
        let id = new_id();
        let now = now_iso();
        let inserted = sqlx::query(
            "INSERT INTO cluster_group_request (id, clusterGroupId, userId, createdAt)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(group_id)
        .bind(target_id)
        .bind(&now)
        .execute(self.pool())
        .await;
        if let Err(error) = inserted {
            if is_unique(&error) {
                let Some(row) = self.request_for_pair(group_id, target_id).await? else {
                    return Err(error.into());
                };
                return Ok(ClusterRequest {
                    created: false,
                    document: request_json(&row),
                });
            }
            return Err(error.into());
        }
        let sender_name: String = sqlx::query_scalar("SELECT name FROM user WHERE id = ?")
            .bind(user_id)
            .fetch_one(self.pool())
            .await?;
        let description = format!("{sender_name} asked you to join their cluster group");
        self.create_notification(
            target_id,
            "ClusterGroupRequest",
            "Cluster Group Request",
            Some(&description),
            Some(json!({ "clusterGroupId": group_id })),
        )
        .await?;
        Ok(ClusterRequest {
            created: true,
            document: json!({
                "id": id,
                "clusterGroupId": group_id,
                "userId": target_id,
                "createdAt": now,
            }),
        })
    }

    pub(crate) async fn accept_cluster_request(
        &self,
        user_id: &str,
        request_id: &str,
    ) -> Result<(), ClusterFail> {
        let Some(request) = self.owned_request(request_id, user_id).await? else {
            return Err(ClusterFail::BadRequest(REQUEST_READ_MISS));
        };
        let mut tx = self.pool().begin().await?;
        let still: Option<RequestRow> = sqlx::query_as(
            "SELECT id, clusterGroupId AS cluster_group_id, userId AS user_id, createdAt AS created_at
             FROM cluster_group_request WHERE id = ? AND userId = ?",
        )
        .bind(request_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(still) = still else {
            return Err(ClusterFail::BadRequest("Request not found"));
        };
        reassign_people(&mut tx, user_id, &still.cluster_group_id).await?;
        sqlx::query("UPDATE user SET clusterGroupId = ?, updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(&still.cluster_group_id)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM cluster_group_request WHERE id = ?")
            .bind(&request.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn delete_cluster_request(
        &self,
        user_id: &str,
        request_id: &str,
    ) -> Result<(), ClusterFail> {
        let removed = sqlx::query(
            "DELETE FROM cluster_group_request
             WHERE id = ?
               AND (
                 userId = ?
                 OR clusterGroupId = (SELECT clusterGroupId FROM user WHERE id = ?)
               )",
        )
        .bind(request_id)
        .bind(user_id)
        .bind(user_id)
        .execute(self.pool())
        .await?;
        if removed.rows_affected() == 0 {
            return Err(ClusterFail::BadRequest(DELETE_MISS));
        }
        Ok(())
    }

    pub(crate) async fn leave_cluster_group(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> Result<(), ClusterFail> {
        if !self.is_member(user_id, group_id).await? {
            return Err(ClusterFail::BadRequest(LEAVE_MISS));
        }
        let others: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user
             WHERE clusterGroupId = ? AND id != ? AND deletedAt IS NULL",
        )
        .bind(group_id)
        .bind(user_id)
        .fetch_one(self.pool())
        .await?;
        if others < 1 {
            return Err(ClusterFail::BadRequest(
                "Cannot leave a cluster group without any other members",
            ));
        }
        let mut tx = self.pool().begin().await?;
        let new_group = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "INSERT INTO cluster_group (id, name, createdAt, updatedAt, updateId)
             VALUES (?, NULL, ?, ?, ?)",
        )
        .bind(&new_group)
        .bind(&now)
        .bind(&now)
        .bind(&update_id)
        .execute(&mut *tx)
        .await?;
        reassign_people(&mut tx, user_id, &new_group).await?;
        sqlx::query("UPDATE user SET clusterGroupId = ?, updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(&new_group)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Access matches a group read. The Pi does not run a face model, so this does not queue a job.
    pub(crate) async fn regenerate_cluster_people(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> Result<(), ClusterFail> {
        if !self.can_read_group(user_id, group_id).await? {
            return Err(ClusterFail::BadRequest(READ_MISS));
        }
        Ok(())
    }

    async fn is_member(&self, user_id: &str, group_id: &str) -> Result<bool, ClusterFail> {
        let found: Option<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE id = ? AND clusterGroupId = ?")
                .bind(user_id)
                .bind(group_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(found.is_some())
    }

    async fn is_invited(&self, user_id: &str, group_id: &str) -> Result<bool, ClusterFail> {
        let found: Option<String> = sqlx::query_scalar(
            "SELECT id FROM cluster_group_request WHERE userId = ? AND clusterGroupId = ?",
        )
        .bind(user_id)
        .bind(group_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(found.is_some())
    }

    async fn can_read_group(&self, user_id: &str, group_id: &str) -> Result<bool, ClusterFail> {
        if self.is_member(user_id, group_id).await? {
            return Ok(true);
        }
        self.is_invited(user_id, group_id).await
    }

    async fn request_for_pair(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<Option<RequestRow>, ClusterFail> {
        Ok(sqlx::query_as(
            "SELECT id, clusterGroupId AS cluster_group_id, userId AS user_id, createdAt AS created_at
             FROM cluster_group_request WHERE clusterGroupId = ? AND userId = ?",
        )
        .bind(group_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?)
    }

    async fn owned_request(
        &self,
        request_id: &str,
        user_id: &str,
    ) -> Result<Option<RequestRow>, ClusterFail> {
        Ok(sqlx::query_as(
            "SELECT id, clusterGroupId AS cluster_group_id, userId AS user_id, createdAt AS created_at
             FROM cluster_group_request WHERE id = ? AND userId = ?",
        )
        .bind(request_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?)
    }
}

async fn request_rows(app: &App, sql: &str, id: &str) -> Result<Vec<RequestRow>, ClusterFail> {
    Ok(sqlx::query_as(sql).bind(id).fetch_all(app.pool()).await?)
}

async fn reassign_people(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
    cluster_id: &str,
) -> Result<(), ClusterFail> {
    let owned: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT personGroupId FROM person WHERE ownerId = ?")
            .bind(user_id)
            .fetch_all(&mut **tx)
            .await?;
    for group_id in owned {
        let shared: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM person WHERE personGroupId = ? AND ownerId != ?",
        )
        .bind(&group_id)
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await?;
        if shared == 0 {
            sqlx::query(
                "UPDATE person_group SET clusterGroupId = ?, updatedAt = ?, updateId = ? WHERE id = ?",
            )
            .bind(cluster_id)
            .bind(now_iso())
            .bind(new_update_id())
            .bind(&group_id)
            .execute(&mut **tx)
            .await?;
            continue;
        }
        let new_group = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&new_group)
        .bind(cluster_id)
        .bind(&now)
        .bind(&update_id)
        .bind(&now)
        .bind(&update_id)
        .execute(&mut **tx)
        .await?;
        let person_update = new_update_id();
        sqlx::query(
            "UPDATE person SET personGroupId = ?, updatedAt = ?, updateId = ?
             WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(&new_group)
        .bind(&now)
        .bind(&person_update)
        .bind(user_id)
        .bind(&group_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE asset_face SET personGroupId = ?, updatedAt = ?, updateId = ?
             WHERE personGroupId = ? AND assetId IN (SELECT id FROM asset WHERE ownerId = ?)",
        )
        .bind(&new_group)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(&group_id)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn request_json(row: &RequestRow) -> Value {
    json!({
        "id": row.id,
        "clusterGroupId": row.cluster_group_id,
        "userId": row.user_id,
        "createdAt": row.created_at,
    })
}

fn public_user_json(row: &PublicUser) -> Value {
    let color = row
        .avatar_color
        .clone()
        .unwrap_or_else(|| avatar_color_for_email(&row.email).to_string());
    json!({
        "id": row.id,
        "name": row.name,
        "email": row.email,
        "profileImagePath": row.profile_image_path,
        "avatarColor": color,
        "profileChangedAt": row.profile_changed_at,
    })
}

fn is_uuid_v4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version() == Some(uuid::Version::Random))
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
