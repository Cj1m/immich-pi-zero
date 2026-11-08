//! People the web page lists. A person with no visible timeline face stays off the list.

use chrono::{NaiveDate, Utc};
use http::StatusCode;
use serde_json::{json, Value};

use crate::db::{avatar_color_for_email, new_update_id, now_iso, App, DbError};

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
