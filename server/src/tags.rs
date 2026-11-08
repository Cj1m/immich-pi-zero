//! Tags and the asset pairs the tags screen reads. A tag change refreshes `asset_exif.tags` and the sidecar.

use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};
use crate::duplicates::append_locked;

#[derive(Debug, sqlx::FromRow)]
struct TagRow {
    id: String,
    value: String,
    created_at: String,
    updated_at: String,
    color: Option<String>,
    parent_id: Option<String>,
}

impl App {
    pub async fn list_tags(&self, user_id: &str) -> Result<Value, DbError> {
        let rows = sqlx::query_as::<_, TagRow>(
            "SELECT id, value, createdAt AS created_at, updatedAt AS updated_at,
                    color, parentId AS parent_id
             FROM tag WHERE userId = ? ORDER BY value",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(rows.iter().map(tag_json).collect()))
    }

    pub async fn get_tag(&self, user_id: &str, id: &str) -> Result<Value, DbError> {
        self.require_tag(user_id, id, "tag.read").await?;
        let row = self.tag_row(id).await?;
        Ok(tag_json(&row))
    }

    pub async fn create_tag(&self, user_id: &str, body: &Value) -> Result<Value, DbError> {
        let name = tag_name(body.get("name"))?;
        let color = optional_color(body.get("color"))?;
        let parent_id = optional_uuid(body.get("parentId"))?;
        let value = if let Some(parent_id) = &parent_id {
            self.require_tag(user_id, parent_id, "tag.read").await?;
            let parent = self.tag_row(parent_id).await?;
            format!("{}/{}", parent.value, name)
        } else {
            name.to_string()
        };
        if self.tag_by_value(user_id, &value).await?.is_some() {
            return Err(DbError::BadRequest("A tag with that name already exists"));
        }
        let id = self
            .insert_tag(user_id, &value, parent_id.as_deref(), color.as_deref())
            .await?;
        Ok(tag_json(&self.tag_row(&id).await?))
    }

    pub async fn update_tag(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        self.require_tag(user_id, id, "tag.update").await?;
        let existing = self.tag_row(id).await?;
        let name = match body.get("name") {
            Some(value) => Some(tag_name(Some(value))?),
            None => None,
        };
        let color = match body.get("color") {
            Some(value) => Some(optional_color(Some(value))?),
            None => None,
        };
        let new_value = match name {
            Some(name) => match existing.value.rfind('/') {
                Some(index) => format!("{}/{}", &existing.value[..index], name),
                None => name.to_string(),
            },
            None => existing.value.clone(),
        };
        if new_value != existing.value && self.tag_by_value(user_id, &new_value).await?.is_some() {
            return Err(DbError::BadRequest("A tag with that name already exists"));
        }
        let now = now_iso();
        let update_id = new_update_id();
        if let Some(color) = &color {
            sqlx::query(
                "UPDATE tag SET value = ?, color = ?, updatedAt = ?, updateId = ? WHERE id = ?",
            )
            .bind(&new_value)
            .bind(color)
            .bind(&now)
            .bind(&update_id)
            .bind(id)
            .execute(self.pool())
            .await?;
        } else {
            sqlx::query("UPDATE tag SET value = ?, updatedAt = ?, updateId = ? WHERE id = ?")
                .bind(&new_value)
                .bind(&now)
                .bind(&update_id)
                .bind(id)
                .execute(self.pool())
                .await?;
        }
        if new_value != existing.value {
            let prefix_len = existing.value.len() as i64;
            sqlx::query(
                "UPDATE tag
                 SET value = ? || substr(value, ?), updatedAt = ?, updateId = ?
                 WHERE userId = ?
                   AND substr(value, 1, ?) = ?
                   AND substr(value, ?, 1) = '/'",
            )
            .bind(&new_value)
            .bind(prefix_len + 1)
            .bind(&now)
            .bind(new_update_id())
            .bind(user_id)
            .bind(prefix_len)
            .bind(&existing.value)
            .bind(prefix_len + 1)
            .execute(self.pool())
            .await?;
            let asset_ids = self.asset_ids_for_tag(id).await?;
            for asset_id in &asset_ids {
                self.store_asset_tags(asset_id).await?;
                self.write_tag_sidecar(asset_id).await;
            }
        }
        Ok(tag_json(&self.tag_row(id).await?))
    }

    pub async fn upsert_tags(&self, user_id: &str, body: &Value) -> Result<Value, DbError> {
        let Some(tags) = body.get("tags").and_then(Value::as_array) else {
            return Err(DbError::Validation);
        };
        let mut unique = Vec::new();
        for tag in tags {
            let Some(tag) = tag.as_str() else {
                return Err(DbError::Validation);
            };
            if !unique.iter().any(|existing: &String| existing == tag) {
                unique.push(tag.to_string());
            }
        }
        let mut documents = Vec::new();
        for tag in unique {
            let parts: Vec<&str> = tag.split('/').filter(|part| !part.is_empty()).collect();
            let mut parent_id: Option<String> = None;
            let mut parent_value: Option<String> = None;
            for part in parts {
                if part.contains('/') {
                    return Err(DbError::Validation);
                }
                let value = match &parent_value {
                    Some(parent) => format!("{parent}/{part}"),
                    None => part.to_string(),
                };
                let id = self
                    .upsert_tag_value(user_id, &value, parent_id.as_deref())
                    .await?;
                parent_value = Some(value);
                parent_id = Some(id);
            }
            if let Some(id) = parent_id {
                documents.push(tag_json(&self.tag_row(&id).await?));
            }
        }
        Ok(Value::Array(documents))
    }

    pub async fn delete_tag(&self, user_id: &str, id: &str) -> Result<(), DbError> {
        self.require_tag(user_id, id, "tag.delete").await?;
        let asset_ids = self.asset_ids_for_tag(id).await?;
        sqlx::query("DELETE FROM tag WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        for asset_id in &asset_ids {
            self.store_asset_tags(asset_id).await?;
            self.write_tag_sidecar(asset_id).await;
        }
        Ok(())
    }

    pub async fn bulk_tag_assets(&self, user_id: &str, body: &Value) -> Result<Value, DbError> {
        let tag_ids = uuid_list(body.get("tagIds"))?;
        let asset_ids = uuid_list(body.get("assetIds"))?;
        let mut count = 0;
        let mut touched = Vec::new();
        for tag_id in &tag_ids {
            if !self.owns_tag(user_id, tag_id).await? {
                continue;
            }
            for asset_id in &asset_ids {
                if !self.can_update_asset(user_id, asset_id).await? {
                    continue;
                }
                let inserted = sqlx::query(
                    "INSERT INTO tag_asset (assetId, tagId)
                     SELECT ?, ?
                     WHERE NOT EXISTS (
                       SELECT 1 FROM tag_asset WHERE assetId = ? AND tagId = ?
                     )",
                )
                .bind(asset_id)
                .bind(tag_id)
                .bind(asset_id)
                .bind(tag_id)
                .execute(self.pool())
                .await?;
                if inserted.rows_affected() == 0 {
                    continue;
                }
                count += 1;
                if !touched.iter().any(|id: &String| id == asset_id) {
                    touched.push(asset_id.clone());
                }
            }
        }
        for asset_id in &touched {
            self.store_asset_tags(asset_id).await?;
            self.write_tag_sidecar(asset_id).await;
        }
        Ok(json!({ "count": count }))
    }

    pub async fn add_tag_assets(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        self.require_tag(user_id, id, "tag.asset").await?;
        let asset_ids = uuid_list(body.get("ids"))?;
        let mut results = Vec::new();
        let mut added = Vec::new();
        for asset_id in &asset_ids {
            let linked: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM tag_asset WHERE tagId = ? AND assetId = ?",
            )
            .bind(id)
            .bind(asset_id)
            .fetch_one(self.pool())
            .await?;
            if linked > 0 {
                results.push(json!({ "id": asset_id, "success": false, "error": "duplicate" }));
                continue;
            }
            if !self.can_update_asset(user_id, asset_id).await? {
                results.push(json!({ "id": asset_id, "success": false, "error": "no_permission" }));
                continue;
            }
            sqlx::query("INSERT INTO tag_asset (assetId, tagId) VALUES (?, ?)")
                .bind(asset_id)
                .bind(id)
                .execute(self.pool())
                .await?;
            added.push(asset_id.clone());
            results.push(json!({ "id": asset_id, "success": true }));
        }
        for asset_id in &added {
            self.store_asset_tags(asset_id).await?;
            self.write_tag_sidecar(asset_id).await;
        }
        Ok(Value::Array(results))
    }

    pub async fn remove_tag_assets(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, DbError> {
        self.require_tag(user_id, id, "tag.asset").await?;
        let asset_ids = uuid_list(body.get("ids"))?;
        let mut results = Vec::new();
        let mut removed = Vec::new();
        for asset_id in &asset_ids {
            let deleted = sqlx::query("DELETE FROM tag_asset WHERE tagId = ? AND assetId = ?")
                .bind(id)
                .bind(asset_id)
                .execute(self.pool())
                .await?;
            if deleted.rows_affected() == 0 {
                results.push(json!({ "id": asset_id, "success": false, "error": "not_found" }));
                continue;
            }
            removed.push(asset_id.clone());
            results.push(json!({ "id": asset_id, "success": true }));
        }
        for asset_id in &removed {
            self.store_asset_tags(asset_id).await?;
            self.write_tag_sidecar(asset_id).await;
        }
        Ok(Value::Array(results))
    }

    pub(crate) async fn asset_tag_documents(&self, asset_id: &str) -> Result<Value, DbError> {
        let rows = sqlx::query_as::<_, TagRow>(
            "SELECT tag.id, tag.value, tag.createdAt AS created_at,
                    tag.updatedAt AS updated_at, tag.color, tag.parentId AS parent_id
             FROM tag
             JOIN tag_asset ON tag_asset.tagId = tag.id
             WHERE tag_asset.assetId = ?
             ORDER BY tag.value",
        )
        .bind(asset_id)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(rows.iter().map(tag_json).collect()))
    }

    /// Replace the keeper's tags with the tags on the group that this user owns.
    pub(crate) async fn merge_group_tags(
        &self,
        owner_id: &str,
        keeper: &str,
        asset_ids: &[&str],
    ) -> Result<bool, DbError> {
        if asset_ids.is_empty() {
            return Ok(false);
        }
        let placeholders = std::iter::repeat_n("?", asset_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT tag.id
             FROM tag
             JOIN tag_asset ON tag_asset.tagId = tag.id
             WHERE tag.userId = ? AND tag_asset.assetId IN ({placeholders})
             ORDER BY tag.value"
        );
        let mut query = sqlx::query_scalar::<_, String>(&sql).bind(owner_id);
        for id in asset_ids {
            query = query.bind(id);
        }
        let tag_ids = query.fetch_all(self.pool()).await?;
        if tag_ids.is_empty() {
            return Ok(false);
        }
        sqlx::query("DELETE FROM tag_asset WHERE assetId = ?")
            .bind(keeper)
            .execute(self.pool())
            .await?;
        for tag_id in &tag_ids {
            sqlx::query("INSERT INTO tag_asset (assetId, tagId) VALUES (?, ?)")
                .bind(keeper)
                .bind(tag_id)
                .execute(self.pool())
                .await?;
        }
        self.store_asset_tags(keeper).await?;
        Ok(true)
    }

    async fn store_asset_tags(&self, asset_id: &str) -> Result<(), DbError> {
        let values: Vec<String> = sqlx::query_scalar(
            "SELECT tag.value FROM tag
             JOIN tag_asset ON tag_asset.tagId = tag.id
             WHERE tag_asset.assetId = ?
             ORDER BY tag.value",
        )
        .bind(asset_id)
        .fetch_all(self.pool())
        .await?;
        let tags = serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string());
        let existing: Option<String> =
            sqlx::query_scalar("SELECT lockedProperties FROM asset_exif WHERE assetId = ?")
                .bind(asset_id)
                .fetch_optional(self.pool())
                .await?
                .flatten();
        let locked = append_locked(existing.as_deref(), &["tags"]);
        let now = now_iso();
        sqlx::query(
            "INSERT INTO asset_exif (assetId, description, tags, lockedProperties, updatedAt, updateId)
             VALUES (?, '', ?, ?, ?, ?)
             ON CONFLICT(assetId) DO UPDATE SET
               tags = excluded.tags,
               lockedProperties = excluded.lockedProperties,
               updatedAt = excluded.updatedAt,
               updateId = excluded.updateId",
        )
        .bind(asset_id)
        .bind(tags)
        .bind(locked)
        .bind(&now)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn write_tag_sidecar(&self, asset_id: &str) {
        if let Err(error) = self.write_locked_sidecar(asset_id).await {
            eprintln!("sidecar write: {error:?}");
        }
    }

    async fn upsert_tag_value(
        &self,
        user_id: &str,
        value: &str,
        parent_id: Option<&str>,
    ) -> Result<String, DbError> {
        if let Some(existing) = self.tag_by_value(user_id, value).await? {
            if existing.parent_id.as_deref() != parent_id {
                sqlx::query(
                    "UPDATE tag SET parentId = ?, updatedAt = ?, updateId = ? WHERE id = ?",
                )
                .bind(parent_id)
                .bind(now_iso())
                .bind(new_update_id())
                .bind(&existing.id)
                .execute(self.pool())
                .await?;
                if let Some(parent_id) = parent_id {
                    self.copy_parent_closure(&existing.id, parent_id).await?;
                }
            }
            return Ok(existing.id);
        }
        self.insert_tag(user_id, value, parent_id, None).await
    }

    async fn insert_tag(
        &self,
        user_id: &str,
        value: &str,
        parent_id: Option<&str>,
        color: Option<&str>,
    ) -> Result<String, DbError> {
        let id = new_id();
        let now = now_iso();
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO tag (id, userId, value, createdAt, updatedAt, color, parentId, updateId)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(value)
        .bind(&now)
        .bind(&now)
        .bind(color)
        .bind(parent_id)
        .bind(new_update_id())
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO tag_closure (id_ancestor, id_descendant) VALUES (?, ?)")
            .bind(&id)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        if let Some(parent_id) = parent_id {
            sqlx::query(
                "INSERT OR IGNORE INTO tag_closure (id_ancestor, id_descendant)
                 SELECT id_ancestor, ? FROM tag_closure WHERE id_descendant = ?",
            )
            .bind(&id)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    async fn copy_parent_closure(&self, tag_id: &str, parent_id: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT OR IGNORE INTO tag_closure (id_ancestor, id_descendant)
             SELECT id_ancestor, ? FROM tag_closure WHERE id_descendant = ?",
        )
        .bind(tag_id)
        .bind(parent_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn asset_ids_for_tag(&self, tag_id: &str) -> Result<Vec<String>, DbError> {
        let ids = sqlx::query_scalar(
            "SELECT DISTINCT tag_asset.assetId
             FROM tag_closure
             JOIN tag_asset ON tag_asset.tagId = tag_closure.id_descendant
             WHERE tag_closure.id_ancestor = ?",
        )
        .bind(tag_id)
        .fetch_all(self.pool())
        .await?;
        Ok(ids)
    }

    async fn require_tag(&self, user_id: &str, id: &str, permission: &str) -> Result<(), DbError> {
        if self.owns_tag(user_id, id).await? {
            Ok(())
        } else {
            Err(DbError::BadRequest(permission_message(permission)))
        }
    }

    async fn owns_tag(&self, user_id: &str, id: &str) -> Result<bool, DbError> {
        let found: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM tag WHERE id = ? AND userId = ?")
                .bind(id)
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(found.is_some())
    }

    async fn can_update_asset(&self, owner_id: &str, id: &str) -> Result<bool, DbError> {
        let visibility: Option<String> =
            sqlx::query_scalar("SELECT visibility FROM asset WHERE id = ? AND ownerId = ?")
                .bind(id)
                .bind(owner_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(visibility.is_some_and(|value| value != "locked"))
    }

    async fn tag_row(&self, id: &str) -> Result<TagRow, DbError> {
        let row = sqlx::query_as::<_, TagRow>(
            "SELECT id, value, createdAt AS created_at, updatedAt AS updated_at,
                    color, parentId AS parent_id
             FROM tag WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        row.ok_or(DbError::NotFound)
    }

    async fn tag_by_value(&self, user_id: &str, value: &str) -> Result<Option<TagRow>, DbError> {
        let row = sqlx::query_as::<_, TagRow>(
            "SELECT id, value, createdAt AS created_at, updatedAt AS updated_at,
                    color, parentId AS parent_id
             FROM tag WHERE userId = ? AND value = ?",
        )
        .bind(user_id)
        .bind(value)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }
}

fn tag_json(row: &TagRow) -> Value {
    let mut value = json!({
        "id": row.id,
        "name": row.value.rsplit('/').next().unwrap_or(row.value.as_str()),
        "value": row.value,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    });
    if let Some(parent_id) = &row.parent_id {
        value["parentId"] = json!(parent_id);
    }
    if let Some(color) = &row.color {
        value["color"] = json!(color);
    }
    value
}

fn tag_name(value: Option<&Value>) -> Result<&str, DbError> {
    let Some(name) = value.and_then(Value::as_str) else {
        return Err(DbError::Validation);
    };
    if name.contains('/') {
        return Err(DbError::Validation);
    }
    Ok(name)
}

fn optional_color(value: Option<&Value>) -> Result<Option<String>, DbError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(color) = value.as_str() else {
        return Err(DbError::Validation);
    };
    if !is_hex_color(color) {
        return Err(DbError::Validation);
    }
    Ok(Some(color.to_string()))
}

fn optional_uuid(value: Option<&Value>) -> Result<Option<String>, DbError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(id) = value.as_str() else {
        return Err(DbError::Validation);
    };
    if !is_uuid_v4(id) {
        return Err(DbError::Validation);
    }
    Ok(Some(id.to_string()))
}

fn uuid_list(value: Option<&Value>) -> Result<Vec<String>, DbError> {
    let Some(list) = value.and_then(Value::as_array) else {
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

fn is_hex_color(value: &str) -> bool {
    let digits = value.strip_prefix('#').unwrap_or(value);
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.chars().all(|char| char.is_ascii_hexdigit())
}

fn permission_message(permission: &str) -> &'static str {
    match permission {
        "tag.read" => "Not found or no tag.read access",
        "tag.update" => "Not found or no tag.update access",
        "tag.delete" => "Not found or no tag.delete access",
        "tag.asset" => "Not found or no tag.asset access",
        _ => "Not found or no tag.read access",
    }
}
