//! External libraries. A scan does not read a folder and does not create assets.
//! An upload still stores `libraryId` null.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};

const DEFAULT_EXCLUSIONS: [&str; 6] = [
    "**/@eaDir/**",
    "**/._*",
    "**/#recycle/**",
    "**/#snapshot/**",
    "**/.stversions/**",
    "**/.stfolder/**",
];

#[derive(Debug)]
pub(crate) enum LibraryFail {
    Db(DbError),
    BadRequest(String),
}

impl From<DbError> for LibraryFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for LibraryFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

fn bad(text: impl Into<String>) -> LibraryFail {
    LibraryFail::BadRequest(text.into())
}

#[derive(sqlx::FromRow)]
struct LibraryUsage {
    photos: i64,
    videos: i64,
    usage: i64,
}

#[derive(sqlx::FromRow)]
struct LibraryRow {
    id: String,
    owner_id: String,
    name: String,
    import_paths: String,
    exclusion_patterns: String,
    created_at: String,
    updated_at: String,
    refreshed_at: Option<String>,
}

impl App {
    pub(crate) async fn library_list(&self) -> Result<Value, LibraryFail> {
        let rows: Vec<LibraryRow> = sqlx::query_as(
            "SELECT id, ownerId AS owner_id, name, importPaths AS import_paths,
                    exclusionPatterns AS exclusion_patterns, createdAt AS created_at,
                    updatedAt AS updated_at, refreshedAt AS refreshed_at
             FROM library WHERE deletedAt IS NULL ORDER BY createdAt ASC, id ASC",
        )
        .fetch_all(self.pool())
        .await?;
        let mut libraries = Vec::with_capacity(rows.len());
        for row in rows {
            libraries.push(self.library_document(&row).await?);
        }
        Ok(Value::Array(libraries))
    }

    pub(crate) async fn library_one(&self, id: &str) -> Result<Value, LibraryFail> {
        let row = self.library_row(id).await?;
        self.library_document(&row).await
    }

    pub(crate) async fn create_library(&self, body: &Value) -> Result<Value, LibraryFail> {
        let object = body.as_object().ok_or_else(|| bad("Validation failed"))?;
        let owner_id = object
            .get("ownerId")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("Validation failed"))?;
        if !is_uuid_v4(owner_id) {
            return Err(bad("Validation failed"));
        }
        let owner_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?")
            .bind(owner_id)
            .fetch_optional(self.pool())
            .await?;
        if owner_exists.is_none() {
            return Err(bad("Validation failed"));
        }
        let name = match object.get("name") {
            None | Some(Value::Null) => "New External Library".to_string(),
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            Some(_) => return Err(bad("Validation failed")),
        };
        let import_paths = string_list(body, "importPaths")?.unwrap_or_default();
        let exclusion_patterns = match string_list(body, "exclusionPatterns")? {
            Some(patterns) => patterns,
            None => DEFAULT_EXCLUSIONS
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect(),
        };
        let id = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "INSERT INTO library (
                id, name, ownerId, importPaths, exclusionPatterns, createdAt, updatedAt, updateId
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&name)
        .bind(owner_id)
        .bind(json_list(&import_paths))
        .bind(json_list(&exclusion_patterns))
        .bind(&now)
        .bind(&now)
        .bind(&update_id)
        .execute(self.pool())
        .await?;
        self.library_one(&id).await
    }

    pub(crate) async fn update_library(
        &self,
        id: &str,
        body: &Value,
    ) -> Result<Value, LibraryFail> {
        let row = self.library_row(id).await?;
        let object = body.as_object().ok_or_else(|| bad("Validation failed"))?;
        let name = match object.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) if !name.is_empty() => Some(name.clone()),
            Some(_) => return Err(bad("Validation failed")),
        };
        let import_paths = string_list(body, "importPaths")?;
        if let Some(paths) = &import_paths {
            for path in paths {
                let check = validate_import_path(self.data_dir(), path);
                if check["isValid"].as_bool() != Some(true) {
                    let message = check["message"].as_str().unwrap_or("");
                    return Err(bad(format!("Invalid import path: {message}")));
                }
            }
        }
        let exclusion_patterns = string_list(body, "exclusionPatterns")?;
        if name.is_none() && import_paths.is_none() && exclusion_patterns.is_none() {
            return self.library_document(&row).await;
        }
        let name = name.unwrap_or(row.name);
        let import_paths = import_paths.unwrap_or_else(|| parse_list(&row.import_paths));
        let exclusion_patterns =
            exclusion_patterns.unwrap_or_else(|| parse_list(&row.exclusion_patterns));
        let now = now_iso();
        sqlx::query(
            "UPDATE library
             SET name = ?, importPaths = ?, exclusionPatterns = ?, updatedAt = ?, updateId = ?
             WHERE id = ?",
        )
        .bind(&name)
        .bind(json_list(&import_paths))
        .bind(json_list(&exclusion_patterns))
        .bind(&now)
        .bind(new_update_id())
        .bind(id)
        .execute(self.pool())
        .await?;
        self.library_one(id).await
    }

    pub(crate) async fn delete_library(&self, id: &str) -> Result<(), LibraryFail> {
        self.library_row(id).await?;
        let now = now_iso();
        sqlx::query("UPDATE library SET deletedAt = ?, updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(&now)
            .bind(&now)
            .bind(new_update_id())
            .bind(id)
            .execute(self.pool())
            .await?;
        sqlx::query("DELETE FROM library WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn validate_library(&self, body: &Value) -> Result<Value, LibraryFail> {
        if !body.is_object() && !body.is_null() {
            return Err(bad("Validation failed"));
        }
        let _ = string_list(body, "exclusionPatterns")?;
        let paths = string_list(body, "importPaths")?.unwrap_or_default();
        let import_paths: Vec<Value> = paths
            .iter()
            .map(|path| validate_import_path(self.data_dir(), path))
            .collect();
        Ok(json!({ "importPaths": import_paths }))
    }

    pub(crate) async fn library_statistics(&self, id: &str) -> Result<Value, LibraryFail> {
        match self.library_row(id).await {
            Ok(_) => {}
            Err(LibraryFail::BadRequest(text)) if text == "Library not found" => {
                return Err(bad(format!("Library {id} not found")));
            }
            Err(error) => return Err(error),
        }
        let counts: LibraryUsage = sqlx::query_as(
            "SELECT
                COALESCE(SUM(CASE WHEN asset.type = 'IMAGE' AND asset.visibility != 'hidden'
                    THEN 1 ELSE 0 END), 0) AS photos,
                COALESCE(SUM(CASE WHEN asset.type = 'VIDEO' AND asset.visibility != 'hidden'
                    THEN 1 ELSE 0 END), 0) AS videos,
                COALESCE(SUM(COALESCE(asset_exif.fileSizeInByte, 0)), 0) AS usage
             FROM asset
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             WHERE asset.libraryId = ? AND asset.deletedAt IS NULL",
        )
        .bind(id)
        .fetch_one(self.pool())
        .await?;
        Ok(json!({
            "photos": counts.photos,
            "videos": counts.videos,
            "total": counts.photos + counts.videos,
            "usage": counts.usage,
        }))
    }

    /// Confirm the library exists, then return. No folder is read and no asset is inserted.
    pub(crate) async fn scan_library(&self, id: &str) -> Result<(), LibraryFail> {
        self.library_row(id).await?;
        Ok(())
    }

    async fn library_row(&self, id: &str) -> Result<LibraryRow, LibraryFail> {
        let row: Option<LibraryRow> = sqlx::query_as(
            "SELECT id, ownerId AS owner_id, name, importPaths AS import_paths,
                    exclusionPatterns AS exclusion_patterns, createdAt AS created_at,
                    updatedAt AS updated_at, refreshedAt AS refreshed_at
             FROM library WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        row.ok_or_else(|| bad("Library not found"))
    }

    async fn library_document(&self, row: &LibraryRow) -> Result<Value, LibraryFail> {
        let asset_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM asset WHERE libraryId = ? AND deletedAt IS NULL",
        )
        .bind(&row.id)
        .fetch_one(self.pool())
        .await?;
        Ok(json!({
            "id": row.id,
            "ownerId": row.owner_id,
            "name": row.name,
            "assetCount": asset_count,
            "importPaths": parse_list(&row.import_paths),
            "exclusionPatterns": parse_list(&row.exclusion_patterns),
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "refreshedAt": row.refreshed_at,
        }))
    }
}

fn string_list(body: &Value, key: &str) -> Result<Option<Vec<String>>, LibraryFail> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            if items.len() > 128 {
                return Err(bad("Validation failed"));
            }
            let mut seen = HashSet::new();
            let mut paths = Vec::with_capacity(items.len());
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(bad("Validation failed"));
                };
                if text.trim().is_empty() || !seen.insert(text.to_string()) {
                    return Err(bad("Validation failed"));
                }
                paths.push(text.to_string());
            }
            Ok(Some(paths))
        }
        Some(_) => Err(bad("Validation failed")),
    }
}

fn json_list(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

fn parse_list(text: &str) -> Vec<String> {
    serde_json::from_str(text).unwrap_or_default()
}

fn validate_import_path(media: &Path, import_path: &str) -> Value {
    let resolved = std::path::absolute(import_path).unwrap_or_else(|_| PathBuf::from(import_path));
    if is_media_path(media, &resolved) {
        return invalid(
            import_path,
            "Cannot use media upload folder for external libraries",
        );
    }
    if !Path::new(import_path).is_absolute() {
        return invalid(
            import_path,
            &format!("Import path must be absolute, try {}", resolved.display()),
        );
    }
    match std::fs::metadata(import_path) {
        Ok(meta) if !meta.is_dir() => invalid(import_path, "Not a directory"),
        Ok(_) => match std::fs::read_dir(import_path) {
            Ok(_) => json!({ "importPath": import_path, "isValid": true }),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                invalid(import_path, "Lacking read permission for folder")
            }
            Err(error) => invalid(import_path, &error.to_string()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            invalid(import_path, "Path does not exist (ENOENT)")
        }
        Err(error) => invalid(import_path, &error.to_string()),
    }
}

fn invalid(import_path: &str, message: &str) -> Value {
    json!({
        "importPath": import_path,
        "isValid": false,
        "message": message,
    })
}

fn is_media_path(media: &Path, candidate: &Path) -> bool {
    let mut media = media.to_string_lossy().into_owned();
    let mut candidate = candidate.to_string_lossy().into_owned();
    if !media.ends_with('/') {
        media.push('/');
    }
    if !candidate.ends_with('/') {
        candidate.push('/');
    }
    candidate.starts_with(&media)
}

fn is_uuid_v4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.get_version() == Some(uuid::Version::Random))
}
