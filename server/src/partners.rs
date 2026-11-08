//! Partner library sharing. A row lets the recipient sync the sharer's assets.
//! The main timeline shows those assets only when `inTimeline` is true.

use serde_json::{json, Value};

use crate::db::{avatar_color_for_email, new_update_id, now_iso, App, DbError};

const PARTNER_USER: &str = "
    id, name, email,
    profileImagePath AS profile_image_path,
    avatarColor AS avatar_color,
    profileChangedAt AS profile_changed_at
";

impl App {
    pub(crate) async fn list_partners(
        &self,
        user_id: &str,
        direction: &str,
    ) -> Result<Value, DbError> {
        let shared_by = match direction {
            "shared-by" => true,
            "shared-with" => false,
            _ => return Err(DbError::Validation),
        };
        let (owner_column, other_column) = if shared_by {
            ("partner.sharedById", "partner.sharedWithId")
        } else {
            ("partner.sharedWithId", "partner.sharedById")
        };
        let sql = format!(
            "SELECT partner.inTimeline AS in_timeline,
                    other.id AS id, other.name AS name, other.email AS email,
                    other.profileImagePath AS profile_image_path,
                    other.avatarColor AS avatar_color,
                    other.profileChangedAt AS profile_changed_at
             FROM partner
             JOIN user AS other ON other.id = {other_column} AND other.deletedAt IS NULL
             JOIN user AS owner ON owner.id = {owner_column} AND owner.deletedAt IS NULL
             WHERE {owner_column} = ?
             ORDER BY partner.createdAt, other.id"
        );
        let rows = sqlx::query_as::<_, PartnerRow>(&sql)
            .bind(user_id)
            .fetch_all(self.pool())
            .await?;
        Ok(Value::Array(rows.into_iter().map(partner_json).collect()))
    }

    pub(crate) async fn create_partner(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        if !body.is_object() {
            return Err(DbError::Validation);
        }
        let Some(shared_with) = body.get("sharedWithId").and_then(Value::as_str) else {
            return Err(DbError::Validation);
        };
        self.insert_partner(user_id, shared_with).await
    }

    pub(crate) async fn create_partner_id(
        &self,
        user_id: &str,
        shared_with: &str,
    ) -> Result<Value, DbError> {
        self.insert_partner(user_id, shared_with).await
    }

    pub(crate) async fn update_partner(
        &self,
        user_id: &str,
        shared_by: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        if uuid::Uuid::parse_str(shared_by).is_err() || !body.is_object() {
            return Err(DbError::Validation);
        }
        let Some(in_timeline) = body.get("inTimeline").and_then(Value::as_bool) else {
            return Err(DbError::Validation);
        };
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM partner WHERE sharedById = ? AND sharedWithId = ?")
                .bind(shared_by)
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        if exists.is_none() {
            return Err(DbError::BadRequest("Not found or no partner.update access"));
        }
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "UPDATE partner SET inTimeline = ?, updatedAt = ?, updateId = ?
             WHERE sharedById = ? AND sharedWithId = ?",
        )
        .bind(i64::from(in_timeline))
        .bind(&now)
        .bind(&update_id)
        .bind(shared_by)
        .bind(user_id)
        .execute(self.pool())
        .await?;
        let mut row = self.partner_user(shared_by).await?;
        row.in_timeline = i64::from(in_timeline);
        Ok(partner_json(row))
    }

    pub(crate) async fn remove_partner(
        &self,
        user_id: &str,
        shared_with: &str,
    ) -> Result<(), DbError> {
        if uuid::Uuid::parse_str(shared_with).is_err() {
            return Err(DbError::Validation);
        }
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM partner WHERE sharedById = ? AND sharedWithId = ?")
                .bind(user_id)
                .bind(shared_with)
                .fetch_optional(self.pool())
                .await?;
        if exists.is_none() {
            return Err(DbError::BadRequest("Partner not found"));
        }
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO partner_audit (id, sharedById, sharedWithId, deletedAt) VALUES (?, ?, ?, ?)",
        )
        .bind(new_update_id())
        .bind(user_id)
        .bind(shared_with)
        .bind(now_iso())
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM partner WHERE sharedById = ? AND sharedWithId = ?")
            .bind(user_id)
            .bind(shared_with)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn insert_partner(&self, user_id: &str, shared_with: &str) -> Result<Value, DbError> {
        if uuid::Uuid::parse_str(shared_with).is_err() {
            return Err(DbError::Validation);
        }
        if self.live_partner(user_id, shared_with).await? {
            return Err(DbError::BadRequest("Partner already exists"));
        }
        if self.partner_user_live(shared_with).await?.is_none() {
            return Err(DbError::BadRequest("Invalid user"));
        }
        let now = now_iso();
        let create_id = new_update_id();
        let inserted = sqlx::query(
            "INSERT INTO partner
                (sharedById, sharedWithId, createdAt, createId, updatedAt, inTimeline, updateId)
             VALUES (?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(user_id)
        .bind(shared_with)
        .bind(&now)
        .bind(&create_id)
        .bind(&now)
        .bind(&create_id)
        .execute(self.pool())
        .await;
        if let Err(error) = inserted {
            if is_unique(&error) {
                return Err(DbError::BadRequest("Partner already exists"));
            }
            return Err(error.into());
        }
        let row = self
            .partner_user_live(shared_with)
            .await?
            .ok_or(DbError::BadRequest("Invalid user"))?;
        Ok(partner_json(row))
    }

    async fn live_partner(&self, shared_by: &str, shared_with: &str) -> Result<bool, DbError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM partner
             JOIN user AS shared_by
               ON shared_by.id = partner.sharedById AND shared_by.deletedAt IS NULL
             JOIN user AS shared_with
               ON shared_with.id = partner.sharedWithId AND shared_with.deletedAt IS NULL
             WHERE partner.sharedById = ? AND partner.sharedWithId = ?",
        )
        .bind(shared_by)
        .bind(shared_with)
        .fetch_optional(self.pool())
        .await?;
        Ok(found.is_some())
    }

    async fn partner_user_live(&self, id: &str) -> Result<Option<PartnerRow>, DbError> {
        let sql = format!(
            "SELECT 0 AS in_timeline, {PARTNER_USER}
             FROM user WHERE id = ? AND deletedAt IS NULL"
        );
        Ok(sqlx::query_as::<_, PartnerRow>(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await?)
    }

    async fn partner_user(&self, id: &str) -> Result<PartnerRow, DbError> {
        let sql = format!("SELECT 0 AS in_timeline, {PARTNER_USER} FROM user WHERE id = ?");
        sqlx::query_as::<_, PartnerRow>(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await?
            .ok_or(DbError::BadRequest("Not found or no partner.update access"))
    }
}

#[derive(sqlx::FromRow)]
struct PartnerRow {
    in_timeline: i64,
    id: String,
    name: String,
    email: String,
    profile_image_path: String,
    avatar_color: Option<String>,
    profile_changed_at: String,
}

fn partner_json(row: PartnerRow) -> Value {
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
        "inTimeline": row.in_timeline != 0,
    })
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
