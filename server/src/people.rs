//! People the web page lists. A person with no visible timeline face stays off the list.
//! Merge and face moves use rows that already exist. Nothing here detects a face.

use chrono::{NaiveDate, Utc};
use http::StatusCode;
use serde_json::{json, Value};

use crate::db::{avatar_color_for_email, new_id, new_update_id, now_iso, App, DbError};
use crate::thumbs::FaceCrop;

const MINIMUM_FACES: i64 = 3;

pub(crate) enum PeopleError {
    Db(DbError),
    Status(StatusCode, &'static str),
}

impl From<DbError> for PeopleError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for PeopleError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

pub(crate) struct PeopleQuery {
    pub with_hidden: bool,
    pub page: i64,
    pub size: i64,
    pub favorite: Option<i64>,
    pub hidden: Option<i64>,
    pub name: Option<String>,
    pub shared_by: Option<String>,
    pub shared_with: Option<String>,
}

pub(crate) struct PeopleUserQuery {
    pub person_id: Option<String>,
    pub shared_by: Option<String>,
    pub shared_with: Option<String>,
    pub role: Option<String>,
    /// `None` is either direction. `Some(true)` is shares the caller gave.
    pub shared_by_caller: Option<bool>,
}

#[derive(sqlx::FromRow)]
struct PersonRow {
    person_group_id: String,
    name: String,
    birth_date: Option<String>,
    thumbnail_path: String,
    is_hidden: i64,
    is_favorite: i64,
    color: Option<String>,
    updated_at: String,
}

#[derive(sqlx::FromRow)]
struct ShareRow {
    person_id: String,
    shared_by_id: String,
    shared_with_id: String,
    role: String,
    by_id: String,
    by_name: String,
    by_email: String,
    by_profile_image_path: String,
    by_avatar_color: Option<String>,
    by_profile_changed_at: String,
    with_id: String,
    with_name: String,
    with_email: String,
    with_profile_image_path: String,
    with_avatar_color: Option<String>,
    with_profile_changed_at: String,
}

impl App {
    pub(crate) async fn list_people(
        &self,
        user_id: &str,
        query: &PeopleQuery,
    ) -> Result<Value, PeopleError> {
        let hide_page = i64::from(!query.with_hidden && query.hidden.is_none());
        let page_sql = format!(
            "{PEOPLE}
             AND (? = 0 OR person.isHidden = 0)
             ORDER BY person.isHidden ASC, person.isFavorite DESC, person.name = '',
                      faces.faceCount DESC, person.name, person.createdAt, person.personGroupId
             LIMIT ? OFFSET ?"
        );
        let rows: Vec<PersonRow> = bind_people(sqlx::query_as(&page_sql), user_id, query)
            .bind(hide_page)
            .bind(query.size + 1)
            .bind((query.page - 1) * query.size)
            .fetch_all(self.pool())
            .await?;
        let has_next = rows.len() > query.size as usize;
        let people: Vec<Value> = rows
            .into_iter()
            .take(query.size as usize)
            .map(person_json)
            .collect();
        let count_sql = format!(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN is_hidden = 1 THEN 1 ELSE 0 END), 0) FROM ({PEOPLE}) AS listed"
        );
        let counts: (i64, i64) = bind_people(sqlx::query_as(&count_sql), user_id, query)
            .fetch_one(self.pool())
            .await?;
        Ok(json!({
            "people": people,
            "total": counts.0,
            "hidden": counts.1,
            "hasNextPage": has_next,
        }))
    }

    pub(crate) async fn person_document(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<Value, PeopleError> {
        let Some(row) = self.person_row(user_id, id).await? else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no person.read access",
            ));
        };
        Ok(person_json(row))
    }

    pub(crate) async fn person_statistics(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<Value, PeopleError> {
        if self.person_row(user_id, id).await?.is_none() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no person.read access",
            ));
        }
        let assets: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT assetId) FROM asset_face
             WHERE ownerId = ? AND personGroupId = ? AND deletedAt IS NULL
               AND isVisible = 1 AND assetVisibility = 'timeline' AND assetDeletedAt IS NULL",
        )
        .bind(user_id)
        .bind(id)
        .fetch_one(self.pool())
        .await?;
        Ok(json!({ "assets": assets }))
    }

    pub(crate) async fn person_thumbnail(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<(String, Vec<u8>), PeopleError> {
        let own: Option<String> = sqlx::query_scalar(
            "SELECT thumbnailPath FROM person WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        let shared: Option<String> = sqlx::query_scalar(
            "SELECT person.thumbnailPath
             FROM person
             JOIN person_user ON person_user.personGroupId = person.personGroupId
               AND person_user.sharedById = person.ownerId
               AND person_user.sharedWithId = ?
             WHERE person.personGroupId = ? AND person.thumbnailPath != ''
             LIMIT 1",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        let path = match own {
            Some(path) if !path.is_empty() => Some(path),
            Some(_) => shared.clone(),
            None if shared.is_some() => shared,
            None => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Not found or no person.read access",
                ));
            }
        };
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            return Err(PeopleError::Status(StatusCode::NOT_FOUND, "Not found"));
        };
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| PeopleError::Status(StatusCode::NOT_FOUND, "Not found"))?;
        Ok((thumbnail_type(&path).to_string(), bytes))
    }

    pub(crate) async fn search_people(
        &self,
        user_id: &str,
        name: &str,
        with_hidden: bool,
    ) -> Result<Value, PeopleError> {
        let rows: Vec<PersonRow> = sqlx::query_as(
            "SELECT personGroupId AS person_group_id, name, birthDate AS birth_date,
                    thumbnailPath AS thumbnail_path, isHidden AS is_hidden, isFavorite AS is_favorite,
                    color, updatedAt AS updated_at
             FROM person
             WHERE ownerId = ?
               AND (? = 1 OR isHidden = 0)
               AND instr(lower(name), lower(?)) > 0
             ORDER BY name, personGroupId
             LIMIT 100",
        )
        .bind(user_id)
        .bind(i64::from(with_hidden))
        .bind(name)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(rows.into_iter().map(person_json).collect()))
    }

    pub(crate) async fn update_person(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, PeopleError> {
        let object =
            body.as_object()
                .filter(|object| !object.is_empty())
                .ok_or(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))?;
        let target = match object.get("userId") {
            None => None,
            Some(Value::String(value)) if uuid::Uuid::parse_str(value).is_ok() => {
                Some(value.clone())
            }
            Some(_) => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        let name = match object.get("name") {
            None => None,
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        let birth_date = optional_birth(object.get("birthDate"))?;
        let hidden = optional_bool(body, "isHidden")?;
        let favorite = optional_bool(body, "isFavorite")?;
        let color = optional_text(object.get("color"))?;
        let personal = hidden.is_some()
            || favorite.is_some()
            || color.is_some()
            || object.contains_key("featureFaceAssetId");
        let owner = target.as_deref().unwrap_or(user_id);
        if owner != user_id && personal {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Only name and birthDate can be updated for other users",
            ));
        }
        if name.is_none()
            && birth_date.is_none()
            && hidden.is_none()
            && favorite.is_none()
            && color.is_none()
        {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        }
        if self.person_row(owner, id).await?.is_none() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no person.update access",
            ));
        }
        let birth_value = birth_date.clone().flatten();
        let color_value = color.clone().flatten();
        sqlx::query(
            "UPDATE person
             SET name = CASE WHEN ? = 1 THEN ? ELSE name END,
                 birthDate = CASE WHEN ? = 1 THEN ? ELSE birthDate END,
                 isHidden = COALESCE(?, isHidden),
                 isFavorite = COALESCE(?, isFavorite),
                 color = CASE WHEN ? = 1 THEN ? ELSE color END,
                 updatedAt = ?,
                 updateId = ?
             WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(i64::from(name.is_some()))
        .bind(name.as_deref())
        .bind(i64::from(birth_date.is_some()))
        .bind(birth_value)
        .bind(hidden)
        .bind(favorite)
        .bind(i64::from(color.is_some()))
        .bind(color_value)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(owner)
        .bind(id)
        .execute(self.pool())
        .await?;
        self.person_document(owner, id)
            .await
            .map_err(|error| match error {
                PeopleError::Status(_, "Not found or no person.read access") => {
                    PeopleError::Status(
                        StatusCode::BAD_REQUEST,
                        "Not found or no person.update access",
                    )
                }
                other => other,
            })
    }

    pub(crate) async fn list_person_users(
        &self,
        user_id: &str,
        query: &PeopleUserQuery,
    ) -> Result<Value, PeopleError> {
        let either = i64::from(query.shared_by_caller.is_none());
        let gave = i64::from(query.shared_by_caller == Some(true));
        let received = i64::from(query.shared_by_caller == Some(false));
        let rows: Vec<ShareRow> = sqlx::query_as(
            "SELECT person_user.personGroupId AS person_id,
                    person_user.sharedById AS shared_by_id,
                    person_user.sharedWithId AS shared_with_id,
                    person_user.role AS role,
                    by_user.id AS by_id, by_user.name AS by_name, by_user.email AS by_email,
                    by_user.profileImagePath AS by_profile_image_path,
                    by_user.avatarColor AS by_avatar_color,
                    by_user.profileChangedAt AS by_profile_changed_at,
                    with_user.id AS with_id, with_user.name AS with_name, with_user.email AS with_email,
                    with_user.profileImagePath AS with_profile_image_path,
                    with_user.avatarColor AS with_avatar_color,
                    with_user.profileChangedAt AS with_profile_changed_at
             FROM person_user
             JOIN user AS by_user ON by_user.id = person_user.sharedById
             JOIN user AS with_user ON with_user.id = person_user.sharedWithId
             WHERE (
                    (? = 1 AND (person_user.sharedById = ? OR person_user.sharedWithId = ?))
                    OR (? = 1 AND person_user.sharedById = ?)
                    OR (? = 1 AND person_user.sharedWithId = ?)
             )
               AND (? IS NULL OR person_user.personGroupId = ?)
               AND (? IS NULL OR person_user.sharedById = ?)
               AND (? IS NULL OR person_user.sharedWithId = ?)
               AND (? IS NULL OR person_user.role = ?)
             ORDER BY person_user.personGroupId, person_user.sharedWithId",
        )
        .bind(either)
        .bind(user_id)
        .bind(user_id)
        .bind(gave)
        .bind(user_id)
        .bind(received)
        .bind(user_id)
        .bind(&query.person_id)
        .bind(&query.person_id)
        .bind(&query.shared_by)
        .bind(&query.shared_by)
        .bind(&query.shared_with)
        .bind(&query.shared_with)
        .bind(&query.role)
        .bind(&query.role)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(rows.into_iter().map(share_json).collect()))
    }

    pub(crate) async fn upsert_people_users(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<(), PeopleError> {
        let object = body.as_object().ok_or(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ))?;
        let shared_with = unique_ids(object.get("sharedWithIds"))?;
        let person_ids = match object.get("personIds") {
            None => None,
            Some(_) => Some(unique_ids(object.get("personIds"))?),
        };
        let share_everyone = match object.get("type") {
            None => false,
            Some(Value::String(value)) if value == "everyone" => true,
            Some(_) => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        let role = match object.get("role") {
            Some(Value::String(value)) if matches!(value.as_str(), "read" | "write" | "admin") => {
                value.clone()
            }
            _ => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        if person_ids.is_none() && !share_everyone {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        }
        if shared_with.iter().any(|id| id == user_id) {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Cannot share a person with yourself",
            ));
        }
        if let Some(ids) = &person_ids {
            for id in ids {
                if self.person_row(user_id, id).await?.is_none() {
                    return Err(PeopleError::Status(
                        StatusCode::BAD_REQUEST,
                        "Not found or no person.update access",
                    ));
                }
            }
        }
        self.require_cluster(user_id, &shared_with).await?;
        let now = now_iso();
        if let Some(ids) = person_ids {
            for shared_with_id in &shared_with {
                for person_id in &ids {
                    sqlx::query(
                        "INSERT INTO person_user
                            (personGroupId, sharedById, sharedWithId, role, createdAt, updatedAt, updateId)
                         VALUES (?, ?, ?, ?, ?, ?, ?)
                         ON CONFLICT(personGroupId, sharedById, sharedWithId) DO UPDATE SET
                            role = excluded.role,
                            updatedAt = excluded.updatedAt,
                            updateId = excluded.updateId",
                    )
                    .bind(person_id)
                    .bind(user_id)
                    .bind(shared_with_id)
                    .bind(&role)
                    .bind(&now)
                    .bind(&now)
                    .bind(new_update_id())
                    .execute(self.pool())
                    .await?;
                }
            }
        } else {
            for shared_with_id in &shared_with {
                sqlx::query(
                    "INSERT INTO person_user
                        (personGroupId, sharedById, sharedWithId, role, createdAt, updatedAt, updateId)
                     SELECT person.personGroupId, person.ownerId, ?, ?, ?, ?, ?
                     FROM person
                     WHERE person.ownerId = ?
                     ON CONFLICT(personGroupId, sharedById, sharedWithId) DO UPDATE SET
                        role = excluded.role,
                        updatedAt = excluded.updatedAt,
                        updateId = excluded.updateId",
                )
                .bind(shared_with_id)
                .bind(&role)
                .bind(&now)
                .bind(&now)
                .bind(new_update_id())
                .bind(user_id)
                .execute(self.pool())
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn delete_people_users(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<(), PeopleError> {
        let Some(items) = body.as_array() else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        };
        let mut pairs = Vec::new();
        for item in items {
            let Some(object) = item.as_object() else {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ));
            };
            let person_id = match object.get("personId").and_then(Value::as_str) {
                Some(id) if uuid::Uuid::parse_str(id).is_ok() => id.to_string(),
                _ => {
                    return Err(PeopleError::Status(
                        StatusCode::BAD_REQUEST,
                        "Validation failed",
                    ))
                }
            };
            let shared_with = match object.get("sharedWithId").and_then(Value::as_str) {
                Some(id) if uuid::Uuid::parse_str(id).is_ok() => id.to_string(),
                _ => {
                    return Err(PeopleError::Status(
                        StatusCode::BAD_REQUEST,
                        "Validation failed",
                    ))
                }
            };
            let owner = match object.get("sharedById") {
                None => user_id.to_string(),
                Some(Value::String(id)) if uuid::Uuid::parse_str(id).is_ok() => id.clone(),
                Some(_) => {
                    return Err(PeopleError::Status(
                        StatusCode::BAD_REQUEST,
                        "Validation failed",
                    ))
                }
            };
            if owner != user_id || self.person_row(user_id, &person_id).await?.is_none() {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Not found or no person.update access",
                ));
            }
            pairs.push((person_id, shared_with));
        }
        for (person_id, shared_with) in pairs {
            sqlx::query(
                "DELETE FROM person_user
                 WHERE personGroupId = ? AND sharedWithId = ? AND sharedById = ?",
            )
            .bind(person_id)
            .bind(shared_with)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    async fn require_cluster(
        &self,
        user_id: &str,
        shared_with: &[String],
    ) -> Result<(), PeopleError> {
        if shared_with.is_empty() {
            return Ok(());
        }
        let cluster =
            sqlx::query_scalar::<_, Option<String>>("SELECT clusterGroupId FROM user WHERE id = ?")
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?
                .flatten();
        let Some(cluster) = cluster else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "All users must be in the same cluster group",
            ));
        };
        for id in shared_with {
            let other = sqlx::query_scalar::<_, Option<String>>(
                "SELECT clusterGroupId FROM user WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(self.pool())
            .await?
            .flatten();
            if other.as_deref() != Some(cluster.as_str()) {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "All users must be in the same cluster group",
                ));
            }
        }
        Ok(())
    }

    pub(crate) async fn create_person(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<Value, PeopleError> {
        let object = body.as_object().ok_or(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ))?;
        let name = match object.get("name") {
            None => String::new(),
            Some(Value::String(value)) => value.clone(),
            Some(_) => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        let birth_date = optional_birth(object.get("birthDate"))?.flatten();
        let hidden = optional_bool(body, "isHidden")?.unwrap_or(0);
        let favorite = optional_bool(body, "isFavorite")?.unwrap_or(0);
        let color = optional_text(object.get("color"))?.flatten();
        if let Some(color) = &color {
            if !is_hex_color(color) {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ));
            }
        }
        let id = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        let cluster: Option<String> =
            sqlx::query_scalar("SELECT clusterGroupId FROM user WHERE id = ?")
                .bind(user_id)
                .fetch_one(self.pool())
                .await?;
        sqlx::query(
            "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(cluster)
        .bind(&now)
        .bind(&update_id)
        .bind(&now)
        .bind(&update_id)
        .execute(self.pool())
        .await?;
        sqlx::query(
            "INSERT INTO person
                (ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden,
                 birthDate, faceAssetId, isFavorite, color, updateId)
             VALUES (?, ?, ?, ?, ?, '', ?, ?, NULL, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(&id)
        .bind(&now)
        .bind(&now)
        .bind(name)
        .bind(hidden)
        .bind(birth_date)
        .bind(favorite)
        .bind(color)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        self.person_document(user_id, &id).await
    }

    pub(crate) async fn merge_people(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<Value, PeopleError> {
        let ids = person_ids(body.get("ids"))?;
        if ids.len() < 2 {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "At least two people are required for merging",
            ));
        }
        if ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Cannot merge a person into themselves",
            ));
        }
        let allowed = self.owned_people(user_id, &ids).await?;
        let mut results: Vec<BulkId> = ids
            .iter()
            .map(|id| BulkId {
                id: id.clone(),
                success: false,
                error: Some(if allowed.iter().any(|owned| owned == id) {
                    "not_found"
                } else {
                    "no_permission"
                }),
            })
            .collect();
        let ordered: Vec<String> = ids
            .into_iter()
            .filter(|id| allowed.iter().any(|owned| owned == id))
            .collect();
        let Some(target_id) = ordered.first().cloned() else {
            return Ok(Value::Array(results.iter().map(bulk_json).collect()));
        };
        let people = self.people_for_merge(&ordered).await?;
        let mut owners = vec![user_id.to_string()];
        for person in &people {
            if !owners.iter().any(|owner| owner == &person.owner_id) {
                owners.push(person.owner_id.clone());
            }
        }
        let mut targets: Vec<MergePerson> = people
            .iter()
            .filter(|person| person.person_group_id == target_id)
            .cloned()
            .collect();
        if targets.iter().any(|person| person.owner_id == user_id) {
            mark_success(&mut results, &target_id);
        }
        let mut removed_thumbs = Vec::new();
        let mut tx = self.pool().begin().await?;
        for source_id in ordered.iter().skip(1) {
            for owner_id in &owners {
                let Some(source) = people
                    .iter()
                    .find(|person| {
                        person.owner_id == *owner_id && person.person_group_id == *source_id
                    })
                    .cloned()
                else {
                    continue;
                };
                let auth_user = owner_id == user_id;
                let target = targets
                    .iter()
                    .find(|person| person.owner_id == *owner_id)
                    .cloned();
                if !auth_user
                    && target.as_ref().is_some_and(|person| {
                        (!person.name.is_empty() && !source.name.is_empty())
                            || (person.birth_date.is_some() && source.birth_date.is_some())
                    })
                {
                    continue;
                }
                let Some(target) = target else {
                    retarget_person(&mut tx, owner_id, source_id, &target_id).await?;
                    move_owner_faces(&mut tx, owner_id, source_id, &target_id).await?;
                    targets.push(MergePerson {
                        owner_id: owner_id.clone(),
                        person_group_id: target_id.clone(),
                        name: source.name,
                        birth_date: source.birth_date,
                        thumbnail_path: source.thumbnail_path,
                    });
                    continue;
                };
                if (!source.name.is_empty() && target.name.is_empty())
                    || (source.birth_date.is_some() && target.birth_date.is_none())
                {
                    let name = if target.name.is_empty() && !source.name.is_empty() {
                        Some(source.name.as_str())
                    } else {
                        None
                    };
                    let birth = if target.birth_date.is_none() {
                        source.birth_date.as_deref()
                    } else {
                        None
                    };
                    copy_person_fields(&mut tx, owner_id, &target_id, name, birth).await?;
                    if let Some(stored) = targets
                        .iter_mut()
                        .find(|person| person.owner_id == *owner_id)
                    {
                        if let Some(name) = name {
                            stored.name = name.to_string();
                        }
                        if birth.is_some() {
                            stored.birth_date = source.birth_date.clone();
                        }
                    }
                }
                move_owner_faces(&mut tx, owner_id, source_id, &target_id).await?;
                if let Some(path) = delete_owned_person(&mut tx, owner_id, source_id).await? {
                    removed_thumbs.push(path);
                }
                if auth_user {
                    mark_success(&mut results, source_id);
                }
            }
        }
        sqlx::query(
            "DELETE FROM person_group
             WHERE NOT EXISTS (
                 SELECT 1 FROM person WHERE person.personGroupId = person_group.id
             )",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        for path in removed_thumbs {
            unlink_under(self.data_dir(), &path).await;
        }
        Ok(Value::Array(results.iter().map(bulk_json).collect()))
    }

    pub(crate) async fn reassign_faces(
        &self,
        user_id: &str,
        person_id: &str,
        body: &Value,
    ) -> Result<Value, PeopleError> {
        if self.person_row(user_id, person_id).await?.is_none() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Person not found",
            ));
        }
        let moves = face_moves(user_id, body)?;
        for item in &moves {
            if !self
                .can_update_person(user_id, &item.owner_id, &item.person_id)
                .await?
            {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Not found or no person.update access",
                ));
            }
        }
        let document = self.person_document(user_id, person_id).await?;
        let mut refresh = Vec::new();
        if self.feature_face(user_id, person_id).await?.is_none() {
            refresh.push(person_id.to_string());
        }
        for item in &moves {
            let faces = self
                .faces_on_asset(user_id, &item.asset_id, &item.person_id)
                .await?;
            for face in faces {
                if !self.owns_face(user_id, &face.id).await? {
                    continue;
                }
                if face.feature_face {
                    refresh.push(item.person_id.clone());
                }
                self.assign_face(&face.id, person_id).await?;
            }
        }
        refresh.sort();
        refresh.dedup();
        for group_id in refresh {
            self.refresh_feature_face(user_id, &group_id).await?;
        }
        Ok(Value::Array(vec![document; moves.len()]))
    }

    pub(crate) async fn reassign_face(
        &self,
        user_id: &str,
        person_id: &str,
        body: &Value,
    ) -> Result<Value, PeopleError> {
        if self.person_row(user_id, person_id).await?.is_none() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Person not found",
            ));
        }
        let face_id = body
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ))?;
        if !self.owns_face(user_id, face_id).await? {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no person.create access",
            ));
        }
        let source = self.face_owner_person(user_id, face_id).await?;
        let destination_feature = self.feature_face(user_id, person_id).await?;
        self.assign_face(face_id, person_id).await?;
        if destination_feature.is_none() {
            self.refresh_feature_face(user_id, person_id).await?;
        }
        if let Some(source) = source.filter(|face| face.feature_face) {
            self.refresh_feature_face(user_id, &source.person_id)
                .await?;
        }
        self.person_document(user_id, person_id).await
    }

    pub(crate) async fn delete_face(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<(), PeopleError> {
        let force = match body.get("force") {
            Some(Value::Bool(value)) => *value,
            _ => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        if !self.owns_face(user_id, id).await? {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no face.delete access",
            ));
        }
        if force {
            let asset_id: String =
                sqlx::query_scalar("SELECT assetId FROM asset_face WHERE id = ?")
                    .bind(id)
                    .fetch_one(self.pool())
                    .await?;
            let now = now_iso();
            sqlx::query(
                "INSERT INTO asset_face_audit (id, assetFaceId, assetId, deletedAt) VALUES (?, ?, ?, ?)",
            )
            .bind(new_update_id())
            .bind(id)
            .bind(asset_id)
            .bind(now)
            .execute(self.pool())
            .await?;
            sqlx::query("DELETE FROM asset_face WHERE id = ?")
                .bind(id)
                .execute(self.pool())
                .await?;
        } else {
            sqlx::query(
                "UPDATE asset_face SET deletedAt = ?, updatedAt = ?, updateId = ? WHERE id = ?",
            )
            .bind(now_iso())
            .bind(now_iso())
            .bind(new_update_id())
            .bind(id)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    async fn owned_people(
        &self,
        user_id: &str,
        ids: &[String],
    ) -> Result<Vec<String>, PeopleError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT personGroupId FROM person WHERE ownerId = ? AND personGroupId IN ({placeholders})"
        );
        let mut query = sqlx::query_scalar(&sql).bind(user_id);
        for id in ids {
            query = query.bind(id);
        }
        Ok(query.fetch_all(self.pool()).await?)
    }

    async fn people_for_merge(&self, ids: &[String]) -> Result<Vec<MergePerson>, PeopleError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT ownerId AS owner_id, personGroupId AS person_group_id, name,
                    birthDate AS birth_date, thumbnailPath AS thumbnail_path
             FROM person WHERE personGroupId IN ({placeholders})
             ORDER BY ownerId, personGroupId"
        );
        let mut query = sqlx::query_as(&sql);
        for id in ids {
            query = query.bind(id);
        }
        Ok(query.fetch_all(self.pool()).await?)
    }

    async fn can_update_person(
        &self,
        user_id: &str,
        owner_id: &str,
        person_id: &str,
    ) -> Result<bool, PeopleError> {
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM person WHERE ownerId = ? AND personGroupId = ? AND ownerId = ?",
        )
        .bind(owner_id)
        .bind(person_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        if owned.is_some() {
            return Ok(true);
        }
        let shared: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM person_user
             WHERE sharedWithId = ? AND personGroupId = ? AND sharedById = ?
               AND role IN ('write', 'admin')",
        )
        .bind(user_id)
        .bind(person_id)
        .bind(owner_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(shared.is_some())
    }

    async fn feature_face(
        &self,
        user_id: &str,
        person_id: &str,
    ) -> Result<Option<String>, PeopleError> {
        let face: Option<String> = sqlx::query_scalar(
            "SELECT faceAssetId FROM person WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(user_id)
        .bind(person_id)
        .fetch_optional(self.pool())
        .await?
        .flatten();
        Ok(face.filter(|id| !id.is_empty()))
    }

    async fn faces_on_asset(
        &self,
        user_id: &str,
        asset_id: &str,
        person_id: &str,
    ) -> Result<Vec<MovingFace>, PeopleError> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT asset_face.id, person.faceAssetId
             FROM asset_face
             LEFT JOIN person ON person.personGroupId = asset_face.personGroupId
               AND person.ownerId = ?
             WHERE asset_face.assetId = ? AND asset_face.personGroupId = ?
               AND asset_face.deletedAt IS NULL",
        )
        .bind(user_id)
        .bind(asset_id)
        .bind(person_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, feature)| MovingFace {
                id: id.clone(),
                feature_face: feature.as_deref() == Some(id.as_str()),
            })
            .collect())
    }

    async fn face_owner_person(
        &self,
        user_id: &str,
        face_id: &str,
    ) -> Result<Option<SourceFace>, PeopleError> {
        let row: Option<(String, Option<String>, String)> = sqlx::query_as(
            "SELECT asset_face.personGroupId, person.faceAssetId, asset_face.id
             FROM asset_face
             LEFT JOIN person ON person.personGroupId = asset_face.personGroupId
               AND person.ownerId = ?
             WHERE asset_face.id = ? AND asset_face.personGroupId IS NOT NULL",
        )
        .bind(user_id)
        .bind(face_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|(person_id, feature, id)| SourceFace {
            person_id,
            feature_face: feature.as_deref() == Some(id.as_str()),
        }))
    }

    async fn owns_face(&self, user_id: &str, face_id: &str) -> Result<bool, PeopleError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM asset_face
             JOIN asset ON asset.id = asset_face.assetId AND asset.deletedAt IS NULL
             WHERE asset_face.id = ? AND asset.ownerId = ?",
        )
        .bind(face_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(found.is_some())
    }

    async fn assign_face(&self, face_id: &str, person_id: &str) -> Result<(), PeopleError> {
        sqlx::query(
            "UPDATE asset_face SET personGroupId = ?, updatedAt = ?, updateId = ? WHERE id = ?",
        )
        .bind(person_id)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(face_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn refresh_feature_face(
        &self,
        user_id: &str,
        person_id: &str,
    ) -> Result<(), PeopleError> {
        let face: Option<FeatureFace> = sqlx::query_as(
            "SELECT id AS face_id, assetId AS asset_id, imageWidth AS width, imageHeight AS height,
                    boundingBoxX1 AS x1, boundingBoxY1 AS y1, boundingBoxX2 AS x2, boundingBoxY2 AS y2
             FROM asset_face
             WHERE personGroupId = ? AND ownerId = ? AND deletedAt IS NULL AND isVisible = 1
             ORDER BY id
             LIMIT 1",
        )
        .bind(person_id)
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(face) = face else {
            return Ok(());
        };
        let face_id = face.face_id;
        let asset_id = face.asset_id;
        sqlx::query(
            "UPDATE person SET faceAssetId = ?, updatedAt = ?, updateId = ?
             WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(&face_id)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(user_id)
        .bind(person_id)
        .execute(self.pool())
        .await?;
        let source = self.face_image_source(&asset_id).await?;
        let Some(source) = source else {
            return Ok(());
        };
        let data_dir = self.data_dir().to_path_buf();
        let owner = user_id.to_string();
        let person = person_id.to_string();
        let written = tokio::task::spawn_blocking(move || {
            crate::thumbs::write_person_jpeg(
                &data_dir,
                &owner,
                &person,
                &source,
                FaceCrop {
                    width: face.width,
                    height: face.height,
                    x1: face.x1,
                    y1: face.y1,
                    x2: face.x2,
                    y2: face.y2,
                },
            )
        })
        .await
        .unwrap_or_else(|error| {
            eprintln!("person thumbnail: {error}");
            None
        });
        let Some(path) = written else {
            eprintln!("person thumbnail: {person_id}");
            return Ok(());
        };
        sqlx::query(
            "UPDATE person SET thumbnailPath = ?, updatedAt = ?, updateId = ?
             WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(path.to_string_lossy().to_string())
        .bind(now_iso())
        .bind(new_update_id())
        .bind(user_id)
        .bind(person_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn face_image_source(
        &self,
        asset_id: &str,
    ) -> Result<Option<std::path::PathBuf>, PeopleError> {
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT asset.type, asset.originalPath,
                    (SELECT path FROM asset_file WHERE assetId = asset.id AND type = 'preview')
             FROM asset WHERE id = ?",
        )
        .bind(asset_id)
        .fetch_optional(self.pool())
        .await?;
        let Some((kind, original, preview)) = row else {
            return Ok(None);
        };
        let preview = preview.map(|path| under_data(self.data_dir(), &path));
        if kind == "VIDEO" {
            return Ok(preview.filter(|path| path.is_file()));
        }
        let original = self.data_dir().join("objects").join(original);
        if original.is_file() {
            return Ok(Some(original));
        }
        Ok(preview.filter(|path| path.is_file()))
    }

    async fn person_row(&self, user_id: &str, id: &str) -> Result<Option<PersonRow>, PeopleError> {
        let row = sqlx::query_as(
            "SELECT personGroupId AS person_group_id, name, birthDate AS birth_date,
                    thumbnailPath AS thumbnail_path, isHidden AS is_hidden, isFavorite AS is_favorite,
                    color, updatedAt AS updated_at
             FROM person WHERE ownerId = ? AND personGroupId = ?",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }
}

const PEOPLE: &str = "
SELECT person.personGroupId AS person_group_id, person.name AS name, person.birthDate AS birth_date,
       person.thumbnailPath AS thumbnail_path, person.isHidden AS is_hidden, person.isFavorite AS is_favorite,
       person.color AS color, person.updatedAt AS updated_at
FROM person
JOIN (
    SELECT personGroupId, COUNT(*) AS faceCount
    FROM asset_face
    WHERE ownerId = ? AND deletedAt IS NULL AND isVisible = 1 AND personGroupId IS NOT NULL
      AND assetVisibility = 'timeline' AND assetDeletedAt IS NULL
    GROUP BY personGroupId
) AS faces ON faces.personGroupId = person.personGroupId
WHERE person.ownerId = ?
  AND (person.name != '' OR faces.faceCount >= ?)
  AND (? IS NULL OR person.isFavorite = ?)
  AND (? IS NULL OR person.isHidden = ?)
  AND (? IS NULL OR instr(lower(person.name), lower(?)) > 0)
  AND (
       (? = 0 AND ? = 0)
       OR EXISTS (
           SELECT 1 FROM person_user
           WHERE person_user.personGroupId = person.personGroupId
             AND (person_user.sharedById = ? OR person_user.sharedWithId = ?)
             AND (? = 0 OR person_user.sharedById = ?)
             AND (? = 0 OR person_user.sharedWithId = ?)
       )
  )
";

fn bind_people<'q, O>(
    query: sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>>,
    user_id: &'q str,
    filters: &'q PeopleQuery,
) -> sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>> {
    let shared_by = i64::from(filters.shared_by.is_some());
    let shared_with = i64::from(filters.shared_with.is_some());
    query
        .bind(user_id)
        .bind(user_id)
        .bind(MINIMUM_FACES)
        .bind(filters.favorite)
        .bind(filters.favorite)
        .bind(filters.hidden)
        .bind(filters.hidden)
        .bind(filters.name.as_deref())
        .bind(filters.name.as_deref())
        .bind(shared_by)
        .bind(shared_with)
        .bind(user_id)
        .bind(user_id)
        .bind(shared_by)
        .bind(filters.shared_by.as_deref())
        .bind(shared_with)
        .bind(filters.shared_with.as_deref())
}

fn person_json(row: PersonRow) -> Value {
    json!({
        "id": row.person_group_id,
        "name": row.name,
        "birthDate": row.birth_date,
        "thumbnailPath": row.thumbnail_path,
        "isHidden": row.is_hidden != 0,
        "isFavorite": row.is_favorite != 0,
        "color": row.color,
        "updatedAt": row.updated_at,
        "otherPeople": [],
        "sharedBy": [],
        "sharedWith": [],
    })
}

fn share_json(row: ShareRow) -> Value {
    json!({
        "personId": row.person_id,
        "sharedById": row.shared_by_id,
        "sharedWithId": row.shared_with_id,
        "role": row.role,
        "sharedBy": user_brief(&row.by_id, &row.by_name, &row.by_email, &row.by_profile_image_path, row.by_avatar_color.as_deref(), &row.by_profile_changed_at),
        "sharedWith": user_brief(&row.with_id, &row.with_name, &row.with_email, &row.with_profile_image_path, row.with_avatar_color.as_deref(), &row.with_profile_changed_at),
    })
}

fn user_brief(
    id: &str,
    name: &str,
    email: &str,
    profile_image_path: &str,
    avatar_color: Option<&str>,
    profile_changed_at: &str,
) -> Value {
    json!({
        "id": id,
        "name": name,
        "email": email,
        "profileImagePath": profile_image_path,
        "avatarColor": avatar_color.unwrap_or_else(|| avatar_color_for_email(email)),
        "profileChangedAt": profile_changed_at,
    })
}

fn thumbnail_type(path: &str) -> &'static str {
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

fn optional_bool(body: &Value, key: &str) -> Result<Option<i64>, PeopleError> {
    match body.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(i64::from(*value))),
        Some(_) => Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        )),
    }
}

fn optional_text(value: Option<&Value>) -> Result<Option<Option<String>>, PeopleError> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(text)) => Ok(Some(Some(text.clone()))),
        Some(_) => Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        )),
    }
}

fn optional_birth(value: Option<&Value>) -> Result<Option<Option<String>>, PeopleError> {
    let Some(text) = optional_text(value)? else {
        return Ok(None);
    };
    let Some(text) = text else {
        return Ok(Some(None));
    };
    let day = text
        .get(..10)
        .filter(|_| text.len() == 10 || text.as_bytes().get(10) == Some(&b'T'));
    let Some(day) = day.and_then(|prefix| NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok())
    else {
        return Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ));
    };
    if day > Utc::now().date_naive() {
        return Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ));
    }
    Ok(Some(Some(day.format("%Y-%m-%d").to_string())))
}

#[derive(Clone, sqlx::FromRow)]
struct MergePerson {
    owner_id: String,
    person_group_id: String,
    name: String,
    birth_date: Option<String>,
    thumbnail_path: String,
}

struct BulkId {
    id: String,
    success: bool,
    error: Option<&'static str>,
}

struct MovingFace {
    id: String,
    feature_face: bool,
}

struct SourceFace {
    person_id: String,
    feature_face: bool,
}

#[derive(sqlx::FromRow)]
struct FeatureFace {
    face_id: String,
    asset_id: String,
    width: i64,
    height: i64,
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
}

struct FaceMove {
    asset_id: String,
    person_id: String,
    owner_id: String,
}

fn person_ids(value: Option<&Value>) -> Result<Vec<String>, PeopleError> {
    let Some(Value::Array(items)) = value else {
        return Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ));
    };
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        };
        if uuid::Uuid::parse_str(text).is_err() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        }
        ids.push(text.to_string());
    }
    Ok(ids)
}

fn face_moves(user_id: &str, body: &Value) -> Result<Vec<FaceMove>, PeopleError> {
    let Some(items) = body.get("data").and_then(Value::as_array) else {
        return Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ));
    };
    let mut moves = Vec::with_capacity(items.len());
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        };
        let Some(asset_id) = object.get("assetId").and_then(Value::as_str) else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        };
        let Some(person_id) = object.get("personId").and_then(Value::as_str) else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        };
        if uuid::Uuid::parse_str(asset_id).is_err() || uuid::Uuid::parse_str(person_id).is_err() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        }
        let owner_id = match object.get("userId") {
            None => user_id.to_string(),
            Some(Value::String(value)) if uuid::Uuid::parse_str(value).is_ok() => value.clone(),
            Some(_) => {
                return Err(PeopleError::Status(
                    StatusCode::BAD_REQUEST,
                    "Validation failed",
                ))
            }
        };
        moves.push(FaceMove {
            asset_id: asset_id.to_string(),
            person_id: person_id.to_string(),
            owner_id,
        });
    }
    Ok(moves)
}

fn mark_success(results: &mut [BulkId], id: &str) {
    if let Some(row) = results.iter_mut().find(|row| row.id == id) {
        row.success = true;
        row.error = None;
    }
}

fn bulk_json(row: &BulkId) -> Value {
    let mut value = json!({ "id": row.id, "success": row.success });
    if let Some(error) = row.error {
        value["error"] = json!(error);
    }
    value
}

async fn retarget_person(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: &str,
    from_group: &str,
    to_group: &str,
) -> Result<(), PeopleError> {
    sqlx::query(
        "UPDATE person SET personGroupId = ?, updatedAt = ?, updateId = ?
         WHERE ownerId = ? AND personGroupId = ?",
    )
    .bind(to_group)
    .bind(now_iso())
    .bind(new_update_id())
    .bind(owner_id)
    .bind(from_group)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn move_owner_faces(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: &str,
    from_group: &str,
    to_group: &str,
) -> Result<(), PeopleError> {
    sqlx::query(
        "UPDATE asset_face SET personGroupId = ?, updatedAt = ?, updateId = ?
         WHERE personGroupId = ? AND assetId IN (SELECT id FROM asset WHERE ownerId = ?)",
    )
    .bind(to_group)
    .bind(now_iso())
    .bind(new_update_id())
    .bind(from_group)
    .bind(owner_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn copy_person_fields(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: &str,
    person_id: &str,
    name: Option<&str>,
    birth_date: Option<&str>,
) -> Result<(), PeopleError> {
    sqlx::query(
        "UPDATE person
         SET name = CASE WHEN ? = 1 THEN ? ELSE name END,
             birthDate = CASE WHEN ? = 1 THEN ? ELSE birthDate END,
             updatedAt = ?,
             updateId = ?
         WHERE ownerId = ? AND personGroupId = ?",
    )
    .bind(i64::from(name.is_some()))
    .bind(name)
    .bind(i64::from(birth_date.is_some()))
    .bind(birth_date)
    .bind(now_iso())
    .bind(new_update_id())
    .bind(owner_id)
    .bind(person_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn delete_owned_person(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: &str,
    person_id: &str,
) -> Result<Option<String>, PeopleError> {
    let path: Option<String> = sqlx::query_scalar(
        "SELECT thumbnailPath FROM person WHERE ownerId = ? AND personGroupId = ?",
    )
    .bind(owner_id)
    .bind(person_id)
    .fetch_optional(&mut **tx)
    .await?;
    let now = now_iso();
    sqlx::query(
        "INSERT INTO person_audit (id, personGroupId, ownerId, deletedAt) VALUES (?, ?, ?, ?)",
    )
    .bind(new_update_id())
    .bind(person_id)
    .bind(owner_id)
    .bind(&now)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM person_user WHERE personGroupId = ? AND sharedById = ?")
        .bind(person_id)
        .bind(owner_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM person WHERE ownerId = ? AND personGroupId = ?")
        .bind(owner_id)
        .bind(person_id)
        .execute(&mut **tx)
        .await?;
    Ok(path.filter(|path| !path.is_empty()))
}

async fn unlink_under(data_dir: &std::path::Path, path: &str) {
    let full = under_data(data_dir, path);
    if !full.starts_with(data_dir) {
        return;
    }
    if let Err(error) = tokio::fs::remove_file(&full).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("person thumbnail: {error}");
        }
    }
}

fn under_data(data_dir: &std::path::Path, path: &str) -> std::path::PathBuf {
    let relative = std::path::Path::new(path);
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return data_dir.join("rejected");
    }
    if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        data_dir.join(relative)
    }
}

fn is_hex_color(value: &str) -> bool {
    let hex = value.strip_prefix('#').unwrap_or(value);
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn unique_ids(value: Option<&Value>) -> Result<Vec<String>, PeopleError> {
    let Some(Value::Array(items)) = value else {
        return Err(PeopleError::Status(
            StatusCode::BAD_REQUEST,
            "Validation failed",
        ));
    };
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        };
        if uuid::Uuid::parse_str(text).is_err() {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        }
        if ids.iter().any(|id: &String| id == text) {
            return Err(PeopleError::Status(
                StatusCode::BAD_REQUEST,
                "Validation failed",
            ));
        }
        ids.push(text.to_string());
    }
    Ok(ids)
}
