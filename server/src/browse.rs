//! Map markers, place search, folders, and the explore groups.

use http::StatusCode;
use serde_json::{json, Value};

use crate::db::{App, DbError};

pub(crate) enum BrowseError {
    Db(DbError),
    Status(StatusCode, &'static str),
}

impl From<DbError> for BrowseError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for BrowseError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

pub(crate) struct MarkerQuery {
    pub archived: bool,
    pub favorite: Option<i64>,
    pub created_after: Option<String>,
    pub created_before: Option<String>,
    pub shared_albums: bool,
}

#[derive(sqlx::FromRow)]
struct MarkerRow {
    id: String,
    latitude: f64,
    longitude: f64,
    city: Option<String>,
    state: Option<String>,
    country: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PlaceRow {
    name: String,
    latitude: f64,
    longitude: f64,
    admin1_name: Option<String>,
    admin2_name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PathRow {
    id: String,
    original_path: String,
    original_file_name: String,
}

impl App {
    pub(crate) async fn library_markers(&self, user_id: &str, query: &MarkerQuery) -> Result<Value, BrowseError> {
        let rows: Vec<MarkerRow> = sqlx::query_as(
            "SELECT asset.id AS id, asset_exif.latitude AS latitude, asset_exif.longitude AS longitude,
                    asset_exif.city AS city, asset_exif.state AS state, asset_exif.country AS country
             FROM asset
             JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.deletedAt IS NULL
               AND asset_exif.latitude IS NOT NULL
               AND asset_exif.longitude IS NOT NULL
               AND (
                    (asset.ownerId = ? AND asset.visibility = 'timeline')
                    OR (? = 1 AND asset.ownerId = ? AND asset.visibility = 'archive')
                    OR (? = 1 AND asset.visibility IN ('timeline', 'archive') AND asset.id IN (
                        SELECT album_asset.assetId FROM album_asset
                        JOIN album_user ON album_user.albumId = album_asset.albumId
                        WHERE album_user.userId = ?
                    ))
               )
               AND (? IS NULL OR asset.isFavorite = ?)
               AND (? IS NULL OR asset.fileCreatedAt >= ?)
               AND (? IS NULL OR asset.fileCreatedAt <= ?)
             ORDER BY asset.fileCreatedAt DESC, asset.id",
        )
        .bind(user_id)
        .bind(i64::from(query.archived))
        .bind(user_id)
        .bind(i64::from(query.shared_albums))
        .bind(user_id)
        .bind(query.favorite)
        .bind(query.favorite)
        .bind(&query.created_after)
        .bind(&query.created_after)
        .bind(&query.created_before)
        .bind(&query.created_before)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "id": row.id,
                        "lat": row.latitude,
                        "lon": row.longitude,
                        "city": row.city,
                        "state": row.state,
                        "country": row.country,
                    })
                })
                .collect(),
        ))
    }

    pub(crate) async fn search_cities(&self, user_id: &str) -> Result<Value, BrowseError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM (
                SELECT asset.id AS id, asset_exif.city AS city,
                       ROW_NUMBER() OVER (
                           PARTITION BY asset_exif.city
                           ORDER BY asset.fileCreatedAt DESC, asset.id
                       ) AS n
                FROM asset
                JOIN asset_exif ON asset_exif.assetId = asset.id
                WHERE asset.ownerId = ?
                  AND asset.deletedAt IS NULL
                  AND asset.visibility = 'timeline'
                  AND asset.type = 'IMAGE'
                  AND asset_exif.city IS NOT NULL
                  AND asset_exif.city != ''
             )
             WHERE n = 1
             ORDER BY city",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(self.asset_documents(user_id, &ids).await?))
    }

    pub(crate) async fn explore(&self, user_id: &str) -> Result<Value, BrowseError> {
        let cities: Vec<(String, String)> = sqlx::query_as(
            "SELECT city, id FROM (
                SELECT asset_exif.city AS city, asset.id AS id,
                       ROW_NUMBER() OVER (
                           PARTITION BY asset_exif.city
                           ORDER BY asset.fileCreatedAt DESC, asset.id
                       ) AS n,
                       COUNT(*) OVER (PARTITION BY asset_exif.city) AS cnt
                FROM asset
                JOIN asset_exif ON asset_exif.assetId = asset.id
                WHERE asset.ownerId = ?
                  AND asset.deletedAt IS NULL
                  AND asset.visibility = 'timeline'
                  AND asset.type = 'IMAGE'
                  AND asset_exif.city IS NOT NULL
                  AND asset_exif.city != ''
             )
             WHERE n = 1 AND cnt >= 5
             ORDER BY cnt DESC, city
             LIMIT 12",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        let mut city_items = Vec::new();
        for (city, id) in cities {
            city_items.push(json!({
                "value": city,
                "data": self.asset_json(user_id, &id).await?,
            }));
        }
        let recents: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, createdAt FROM asset
             WHERE ownerId = ? AND deletedAt IS NULL AND visibility = 'timeline' AND type = 'IMAGE'
             ORDER BY createdAt DESC, id
             LIMIT 12",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        let mut recent_items = Vec::new();
        for (id, created_at) in recents {
            recent_items.push(json!({
                "value": created_at,
                "data": self.asset_json(user_id, &id).await?,
            }));
        }
        Ok(json!([
            { "fieldName": "exifInfo.city", "items": city_items },
            { "fieldName": "createdAt", "items": recent_items },
        ]))
    }

    pub(crate) async fn search_places(&self, name: &str) -> Result<Value, BrowseError> {
        let needle = name.to_lowercase();
        let rows: Vec<PlaceRow> = sqlx::query_as(
            "SELECT name, latitude, longitude, admin1Name AS admin1_name, admin2Name AS admin2_name
             FROM geodata_places
             WHERE instr(lower(name), ?) > 0
                OR instr(lower(COALESCE(admin1Name, '')), ?) > 0
                OR instr(lower(COALESCE(admin2Name, '')), ?) > 0
                OR instr(lower(COALESCE(alternateNames, '')), ?) > 0
             ORDER BY name, id
             LIMIT 20",
        )
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "name": row.name,
                        "latitude": row.latitude,
                        "longitude": row.longitude,
                        "admin1name": row.admin1_name,
                        "admin2name": row.admin2_name,
                    })
                })
                .collect(),
        ))
    }

    pub(crate) async fn large_assets(&self, user_id: &str, body: &Value) -> Result<Value, BrowseError> {
        let min_size = match body.get("minFileSize") {
            None => 0,
            Some(value) => integer(value, 0, i64::MAX)?,
        };
        let size = match body.get("size") {
            None => 250,
            Some(value) => integer(value, 1, 1000)?,
        };
        let visibility = match body.get("visibility") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => {
                return Err(BrowseError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
            }
        };
        if visibility == Some("locked") {
            return Err(BrowseError::Status(
                StatusCode::UNAUTHORIZED,
                "Elevated permission is required",
            ));
        }
        let visibility_sql = match visibility {
            None => "AND asset.visibility NOT IN ('hidden', 'locked')",
            Some("timeline") => "AND asset.visibility = 'timeline'",
            Some("archive") => "AND asset.visibility = 'archive'",
            Some("hidden") => "AND asset.visibility = 'hidden'",
            Some(_) => {
                return Err(BrowseError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
            }
        };
        let ids: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT asset.id FROM asset
             JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.ownerId = ?
               AND asset.deletedAt IS NULL
               AND asset_exif.fileSizeInByte > ?
               {visibility_sql}
             ORDER BY asset_exif.fileSizeInByte DESC, asset.id
             LIMIT ?"
        ))
        .bind(user_id)
        .bind(min_size)
        .bind(size)
        .fetch_all(self.pool())
        .await?;
        Ok(Value::Array(self.asset_documents(user_id, &ids).await?))
    }

    pub(crate) async fn folder_paths(&self, user_id: &str) -> Result<Value, BrowseError> {
        let rows = self.folder_rows(user_id).await?;
        let mut paths = std::collections::BTreeSet::new();
        for row in rows {
            paths.insert(directory(&row.original_path).to_string());
        }
        Ok(Value::Array(paths.into_iter().map(Value::String).collect()))
    }

    pub(crate) async fn folder_assets(&self, user_id: &str, path: &str) -> Result<Value, BrowseError> {
        let wanted = path.trim_end_matches('/');
        let mut rows = self.folder_rows(user_id).await?;
        rows.retain(|row| directory(&row.original_path) == wanted);
        rows.sort_by(|left, right| {
            left.original_file_name
                .cmp(&right.original_file_name)
                .then(left.id.cmp(&right.id))
        });
        let ids: Vec<String> = rows.into_iter().map(|row| row.id).collect();
        Ok(Value::Array(self.asset_documents(user_id, &ids).await?))
    }

    pub(crate) async fn suggestions(
        &self,
        user_id: &str,
        kind: &str,
        include_null: bool,
    ) -> Result<Value, BrowseError> {
        let column = match kind {
            "country" => "country",
            "state" => "state",
            "city" => "city",
            "camera-make" => "make",
            "camera-model" => "model",
            "camera-lens-model" => "lensModel",
            _ => return Err(BrowseError::Status(StatusCode::BAD_REQUEST, "Validation failed")),
        };
        let mut values: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT DISTINCT asset_exif.{column}
             FROM asset_exif
             JOIN asset ON asset.id = asset_exif.assetId
             WHERE asset.ownerId = ?
               AND asset.deletedAt IS NULL
               AND asset.visibility = 'timeline'
               AND asset_exif.{column} IS NOT NULL
               AND asset_exif.{column} != ''
             ORDER BY asset_exif.{column}"
        ))
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        let mut items: Vec<Value> = values.drain(..).map(Value::String).collect();
        if include_null {
            items.push(Value::Null);
        }
        Ok(Value::Array(items))
    }

    async fn folder_rows(&self, user_id: &str) -> Result<Vec<PathRow>, BrowseError> {
        let rows = sqlx::query_as(
            "SELECT id, originalPath AS original_path, originalFileName AS original_file_name
             FROM asset
             WHERE ownerId = ?
               AND deletedAt IS NULL
               AND visibility = 'timeline'
               AND fileCreatedAt IS NOT NULL
               AND fileModifiedAt IS NOT NULL
               AND localDateTime IS NOT NULL",
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    async fn asset_documents(&self, user_id: &str, ids: &[String]) -> Result<Vec<Value>, DbError> {
        let mut documents = Vec::new();
        for id in ids {
            documents.push(self.asset_json(user_id, id).await?);
        }
        Ok(documents)
    }
}

fn directory(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => path[..index].trim_end_matches('/'),
        None => "",
    }
}

fn integer(value: &Value, min: i64, max: i64) -> Result<i64, BrowseError> {
    let Some(number) = value.as_i64() else {
        return Err(BrowseError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
    };
    if number < min || number > max {
        return Err(BrowseError::Status(StatusCode::BAD_REQUEST, "Validation failed"));
    }
    Ok(number)
}
