//! Admin queues, manual jobs, maintenance, integrity, backups, and license keys.
//! A database backup is a SQLite copy. Restoring a `.sql` file does not load it.
//! Starting `videoConversion` writes no encoded video. Face, OCR, and on-Pi models stay off.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::Verifier;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use sha2::Sha256;

use crate::db::{new_id, now_iso, App, DbError};

const QUEUES: [&str; 19] = [
    "thumbnailGeneration",
    "metadataExtraction",
    "videoConversion",
    "faceDetection",
    "facialRecognition",
    "smartSearch",
    "duplicateDetection",
    "backgroundTask",
    "storageTemplateMigration",
    "migration",
    "search",
    "sidecar",
    "library",
    "notifications",
    "backupDatabase",
    "ocr",
    "workflow",
    "integrityCheck",
    "editor",
];

const SERVER_LICENSE_KEY: &str = "LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUlJQklqQU5CZ2txaGtpRzl3MEJBUUVGQUFPQ0FROEFNSUlCQ2dLQ0FRRUFvcG5ZRGEwYS9kVTVJZUc3NGlFRQpNd2RBS2pzTmN6TGRDcVJkMVo5eTVUMndqTzdlWUlPZUpUc2wzNTBzUjBwNEtmU1VEU1h2QzlOcERwYzF0T0tsCjVzaEMvQXhwdlFBTENva0Y0anQ4dnJyZDlmQ2FYYzFUcVJiT21uaGl1Z0Q2dmtyME8vRmIzVURpM1UwVHZoUFAKbFBkdlNhd3pMcldaUExmbUhWVnJiclNLbW45SWVTZ3kwN3VrV1RJeUxzY2lOcnZuQnl3c0phUmVEdW9OV1BCSApVL21vMm1YYThtNHdNV2hpWGVoaUlPUXFNdVNVZ1BlQ3NXajhVVngxQ0dsUnpQREEwYlZOUXZlS1hXVnhjRUk2ClVMRWdKeTJGNDlsSDArYVlDbUJmN05FcjZWUTJXQjk1ZXZUS1hLdm4wcUlNN25nRmxjVUF3NmZ1VjFjTkNUSlMKNndJREFRQUIKLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tDQo=";

const CLIENT_LICENSE_KEY: &str = "LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUlJQklqQU5CZ2txaGtpRzl3MEJBUUVGQUFPQ0FROEFNSUlCQ2dLQ0FRRUF2LzdTMzJjUkE1KysxTm5WRHNDTQpzcFAvakpISU1xT0pYRm5oNE53QTJPcHorUk1mZGNvOTJQc09naCt3d1FlRXYxVTJjMnBqelRpUS8ybHJLcS9rCnpKUmxYd2M0Y1Vlc1FETUpPRitQMnFPTlBiQUprWHZDWFlCVUxpdENJa29Md2ZoU0dOanlJS2FSRGhkL3ROeU4KOCtoTlJabllUMWhTSWo5U0NrS3hVQ096YXRQVjRtQ0RlclMrYkUrZ0VVZVdwOTlWOWF6dkYwRkltblRXcFFTdwpjOHdFWmdPTWg0c3ZoNmFpY3dkemtQQ3dFTGFrMFZhQkgzMUJFVUNRTGI5K0FJdEhBVXRKQ0t4aGI1V2pzMXM5CmJyWGZpMHZycGdjWi82RGFuWTJxZlNQem5PbXZEMkZycmxTMXE0SkpOM1ZvN1d3LzBZeS95TWNtelRXWmhHdWgKVVFJREFRQUIKLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tDQo=";

const STAGING_SERVER_LICENSE_KEY: &str = "LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUlJQklqQU5CZ2txaGtpRzl3MEJBUUVGQUFPQ0FROEFNSUlCQ2dLQ0FRRUE3Sy8yd3ZLUS9NdU8ydi9MUm5saAoyUy9zTHhDOGJiTEw1UUlKOGowQ3BVZW40YURlY2dYMUpKUmtGNlpUVUtpNTdTbEhtS3RSM2JOTzJmdTBUUVg5Ck5WMEJzVzllZVB0MmlTMWl4VVFmTzRObjdvTjZzbEtac01qd29RNGtGRGFmM3VHTlZJc0dMb3UxVWRLUVhpeDEKUlRHcXVTb3NZVjNWRlk3Q1hGYTVWaENBL3poVXNsNGFuVXp3eEF6M01jUFVlTXBaenYvbVZiQlRKVzBPSytWZgpWQUJvMXdYMkVBanpBekVHVzQ3Vko4czhnMnQrNHNPaHFBNStMQjBKVzlORUg5QUpweGZzWE4zSzVtM00yNUJVClZXcTlRYStIdHRENnJ0bnAvcUFweXVkWUdwZk9HYTRCUlZTR1MxMURZM0xrb2FlRzYwUEU5NHpoYjduOHpMWkgKelFJREFRQUIKLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tDQo=";

const STAGING_CLIENT_LICENSE_KEY: &str = "LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUlJQklqQU5CZ2txaGtpRzl3MEJBUUVGQUFPQ0FROEFNSUlCQ2dLQ0FRRUFuSUNyTm5jbGpPSC9JdTNtWVVaRQp0dGJLV1c3OGRuajl5M0U2ekk3dU1NUndEckdYWFhkTGhkUDFxSWtlZHh0clVVeUpCMWR4R04yQW91S082MlNGCldrbU9PTmNGQlRBWFZTdjhUNVY0S0VwWnFQYWEwaXpNaGxMaE5sRXEvY1ZKdllrWlh1Z2x6b1o3cG1nbzFSdHgKam1iRm5NNzhrYTFRUUJqOVdLaEw2eWpWRUl2MDdVS0lKWHBNTnNuS2g1V083MjZhYmMzSE9udTlETjY5VnFFRQo3dGZrUnRWNmx2U1NzMkFVMngzT255cHA4ek53b0lPTWRibGsyb09aWWROZzY0Y3l2SzJoU0FlU3NVMFRyOVc5Ckgra0Y5QlNCNlk0QXl0QlVkSmkrK2pMSW5HM2Q5cU9ieFVzTlYrN05mRkF5NjJkL0xNR0xSOC9OUFc0U0s3c0MKRlFJREFRQUIKLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0tDQo=";

const MAINTENANCE_KEY: &str = "maintenance-mode";
const LICENSE_KEY: &str = "license";

#[derive(Debug)]
pub(crate) enum AdminFail {
    Db(DbError),
    BadRequest(String),
    NotFound,
    Unauthorized,
}

impl From<DbError> for AdminFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for AdminFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

pub(crate) enum MaintenanceOutcome {
    Unchanged,
    Entered(String),
    Ended,
}

struct ReportRow {
    path: String,
    asset_id: Option<String>,
    file_asset_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CsvReport {
    id: String,
    kind: String,
    asset_id: Option<String>,
    file_asset_id: Option<String>,
    path: String,
}

#[derive(sqlx::FromRow)]
struct UserUsage {
    id: String,
    name: String,
    quota_size_in_bytes: Option<i64>,
    photos: i64,
    videos: i64,
    usage: i64,
    usage_photos: i64,
    usage_videos: i64,
}

struct JobCounts {
    active: i64,
    failed: i64,
    delayed: i64,
    waiting: i64,
    paused: i64,
}

fn bad(text: &str) -> AdminFail {
    AdminFail::BadRequest(text.to_string())
}

fn known_queue(name: &str) -> Result<(), AdminFail> {
    if QUEUES.contains(&name) {
        Ok(())
    } else {
        Err(bad("Validation failed"))
    }
}

fn report_type(name: &str) -> bool {
    matches!(
        name,
        "untracked_file" | "missing_file" | "checksum_mismatch"
    )
}

fn valid_backup_name(name: &str) -> bool {
    let stem = name
        .strip_suffix(".sql.gz")
        .or_else(|| name.strip_suffix(".sql"));
    let Some(stem) = stem else {
        return false;
    };
    !stem.is_empty()
        && stem.chars().all(|letter| {
            letter.is_ascii_alphanumeric() || letter == '_' || letter == '-' || letter == '.'
        })
}

fn license_pair(body: &Value) -> Result<(&str, &str), AdminFail> {
    let Some(object) = body.as_object() else {
        return Err(bad("Validation failed"));
    };
    let key = object
        .get("licenseKey")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("Validation failed"))?;
    let activation = object
        .get("activationKey")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("Validation failed"))?;
    Ok((key, activation))
}

fn license_document(key: &str, activation: &str, activated_at: &str) -> Value {
    json!({
        "licenseKey": key,
        "activationKey": activation,
        "activatedAt": activated_at,
    })
}

fn decode_pem(encoded: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn decode_signature(value: &str) -> Option<Vec<u8>> {
    let mut standard = String::with_capacity(value.len() + 4);
    for letter in value.chars() {
        match letter {
            '-' => standard.push('+'),
            '_' => standard.push('/'),
            '\n' | '\r' | ' ' => {}
            other => standard.push(other),
        }
    }
    while standard.len() % 4 != 0 {
        standard.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(standard)
        .ok()
}

/// Unset and `production` use the production keys. `development` and `testing` use the staging keys.
fn license_public_keys() -> (&'static str, &'static str) {
    match std::env::var("IMMICH_ENV") {
        Ok(value) if matches!(value.as_str(), "development" | "testing") => {
            (STAGING_SERVER_LICENSE_KEY, STAGING_CLIENT_LICENSE_KEY)
        }
        _ => (SERVER_LICENSE_KEY, CLIENT_LICENSE_KEY),
    }
}

fn signature_matches(message: &str, activation: &str, public_b64: &str) -> bool {
    let Some(pem) = decode_pem(public_b64) else {
        return false;
    };
    let Ok(key) = rsa::RsaPublicKey::from_public_key_pem(&pem) else {
        return false;
    };
    let Some(bytes) = decode_signature(activation) else {
        return false;
    };
    let Ok(signature) = rsa::pkcs1v15::Signature::try_from(bytes.as_slice()) else {
        return false;
    };
    let verifying = rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key);
    verifying.verify(message.as_bytes(), &signature).is_ok()
}

fn legacy_queue(document: &Value) -> Value {
    let active = document["statistics"]["active"].as_i64().unwrap_or(0);
    json!({
        "queueStatus": {
            "isActive": active > 0,
            "isPaused": document["isPaused"].as_bool().unwrap_or(false),
        },
        "jobCounts": document["statistics"].clone(),
    })
}

fn sha1_file(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(hasher.finalize().to_vec())
}

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let relative = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            collect_files(&path, &relative, out);
        } else if path.is_file() {
            out.push(relative);
        }
    }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn template_html(name: &str, custom: &str) -> String {
    if custom.contains('<') {
        return custom.to_string();
    }
    if custom.is_empty() {
        return format!("<html><body><p>{name}</p></body></html>");
    }
    let escaped = custom
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<html><body><p>{escaped}</p></body></html>")
}

struct MaintenanceState {
    active: bool,
    action: String,
    secret: Option<String>,
    username: String,
}

impl App {
    pub(crate) async fn user_license_value(&self, user_id: &str) -> Result<Value, DbError> {
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT value FROM user_metadata WHERE userId = ? AND key = 'license'",
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(stored
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null))
    }

    pub(crate) async fn queue_list(&self) -> Result<Value, AdminFail> {
        let mut queues = Vec::with_capacity(QUEUES.len());
        for name in QUEUES {
            queues.push(self.queue_document(name).await?);
        }
        Ok(Value::Array(queues))
    }

    pub(crate) async fn queue_one(&self, name: &str) -> Result<Value, AdminFail> {
        known_queue(name)?;
        self.queue_document(name).await
    }

    pub(crate) async fn update_queue(&self, name: &str, body: &Value) -> Result<Value, AdminFail> {
        known_queue(name)?;
        let Some(object) = body.as_object() else {
            return Err(bad("Validation failed"));
        };
        if let Some(flag) = object.get("isPaused") {
            let Some(paused) = flag.as_bool() else {
                return Err(bad("Validation failed"));
            };
            if paused && name == "backgroundTask" {
                return Err(bad("The BackgroundTask queue cannot be paused"));
            }
            self.set_paused(name, paused).await?;
        }
        self.queue_document(name).await
    }

    pub(crate) async fn queue_jobs(
        &self,
        name: &str,
        statuses: &[String],
    ) -> Result<Value, AdminFail> {
        known_queue(name)?;
        for status in statuses {
            if !matches!(
                status.as_str(),
                "active" | "failed" | "completed" | "delayed" | "waiting" | "paused"
            ) {
                return Err(bad("Validation failed"));
            }
        }
        let rows = self.job_rows(name, statuses).await?;
        let mut items = Vec::with_capacity(rows.len());
        for (id, job_name, data, timestamp) in rows {
            let data = serde_json::from_str::<Value>(&data).unwrap_or_else(|_| json!({}));
            items.push(json!({
                "id": id,
                "name": job_name,
                "data": data,
                "timestamp": timestamp,
            }));
        }
        Ok(Value::Array(items))
    }

    pub(crate) async fn empty_queue_jobs(&self, name: &str, body: &Value) -> Result<(), AdminFail> {
        known_queue(name)?;
        let failed = match body.get("failed") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(bad("Validation failed")),
        };
        if !body.is_object() && !body.is_null() {
            return Err(bad("Validation failed"));
        }
        self.delete_job_statuses(name, &["waiting", "delayed", "paused"])
            .await?;
        if failed {
            self.delete_job_statuses(name, &["failed"]).await?;
        }
        Ok(())
    }

    pub(crate) async fn legacy_queues(&self) -> Result<Value, AdminFail> {
        let mut map = serde_json::Map::new();
        for name in QUEUES {
            let document = self.queue_document(name).await?;
            map.insert(name.to_string(), legacy_queue(&document));
        }
        Ok(Value::Object(map))
    }

    pub(crate) async fn queue_command(&self, name: &str, body: &Value) -> Result<Value, AdminFail> {
        known_queue(name)?;
        let Some(object) = body.as_object() else {
            return Err(bad("Validation failed"));
        };
        let Some(command) = object.get("command").and_then(Value::as_str) else {
            return Err(bad("Validation failed"));
        };
        if let Some(force) = object.get("force") {
            if !force.is_boolean() && !force.is_null() {
                return Err(bad("Validation failed"));
            }
        }
        match command {
            "start" => self.start_queue(name).await?,
            "pause" => self.set_paused(name, true).await?,
            "resume" => self.set_paused(name, false).await?,
            "empty" => {
                self.delete_job_statuses(name, &["waiting", "delayed", "paused"])
                    .await?;
            }
            "clear-failed" => self.delete_job_statuses(name, &["failed"]).await?,
            _ => return Err(bad("Validation failed")),
        }
        let document = self.queue_document(name).await?;
        Ok(legacy_queue(&document))
    }

    pub(crate) async fn create_manual_job(&self, body: &Value) -> Result<(), AdminFail> {
        let Some(name) = body.get("name").and_then(Value::as_str) else {
            return Err(bad("Invalid job name"));
        };
        match name {
            "person-cleanup" => self.person_cleanup().await?,
            "tag-cleanup" => self.tag_cleanup().await?,
            "user-cleanup" => self.purge_expired_users().await?,
            "memory-cleanup" => self.cleanup_memories(&now_iso()).await?,
            "memory-create" => {
                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                self.generate_memories(&today).await?;
            }
            "backup-database" => self.write_database_backup().await?,
            "integrity-missing-files" | "integrity-missing-files-refresh" => {
                self.scan_missing().await?;
            }
            "integrity-untracked-files" | "integrity-untracked-files-refresh" => {
                self.scan_untracked().await?;
            }
            "integrity-checksum-mismatch" | "integrity-checksum-mismatch-refresh" => {
                self.scan_checksums().await?;
            }
            "integrity-missing-files-delete-all" => self.delete_report("missing_file").await?,
            "integrity-untracked-files-delete-all" => self.delete_report("untracked_file").await?,
            "integrity-checksum-mismatch-delete-all" => {
                self.delete_report("checksum_mismatch").await?;
            }
            _ => return Err(bad("Invalid job name")),
        }
        Ok(())
    }

    pub(crate) async fn maintenance_status(&self) -> Result<Value, AdminFail> {
        let state = self.read_maintenance().await?;
        if !state.active {
            return Ok(json!({ "active": false, "action": "end" }));
        }
        Ok(json!({ "active": true, "action": state.action }))
    }

    pub(crate) async fn maintenance_login(&self, token: Option<&str>) -> Result<Value, AdminFail> {
        let state = self.read_maintenance().await?;
        if !state.active {
            return Err(bad("Not in maintenance mode"));
        }
        if state.secret.as_deref() != token {
            return Err(AdminFail::Unauthorized);
        }
        Ok(json!({ "username": state.username }))
    }

    /// `end` while maintenance is off changes nothing. Any other action stores the mode.
    /// Ending while the mode is on clears it, because this process does not restart into a worker.
    /// `restore_database` does not load the file into SQLite.
    pub(crate) async fn set_maintenance(
        &self,
        username: &str,
        body: &Value,
    ) -> Result<MaintenanceOutcome, AdminFail> {
        let Some(object) = body.as_object() else {
            return Err(bad("Validation failed"));
        };
        let Some(action) = object.get("action").and_then(Value::as_str) else {
            return Err(bad("Validation failed"));
        };
        if !matches!(
            action,
            "start" | "end" | "select_database_restore" | "restore_database"
        ) {
            return Err(bad("Validation failed"));
        }
        if action == "restore_database" {
            let filename = object
                .get("restoreBackupFilename")
                .and_then(Value::as_str)
                .unwrap_or("");
            if filename.is_empty() {
                return Err(bad("Validation failed"));
            }
        }
        let state = self.read_maintenance().await?;
        if action == "end" {
            if !state.active {
                return Ok(MaintenanceOutcome::Unchanged);
            }
            sqlx::query("DELETE FROM system_metadata WHERE key = ?")
                .bind(MAINTENANCE_KEY)
                .execute(self.pool())
                .await?;
            return Ok(MaintenanceOutcome::Ended);
        }
        let secret = if state.active {
            state.secret.unwrap_or_else(new_id)
        } else {
            new_id()
        };
        let stored_user = if state.active && !state.username.is_empty() {
            state.username
        } else {
            username.to_string()
        };
        let document = json!({
            "isMaintenanceMode": true,
            "secret": secret,
            "username": stored_user,
            "action": action,
        })
        .to_string();
        self.write_system_metadata(MAINTENANCE_KEY, &document)
            .await?;
        Ok(MaintenanceOutcome::Entered(secret))
    }

    pub(crate) async fn list_backups(&self) -> Result<Value, AdminFail> {
        let dir = self.backup_dir();
        let timezone = chrono::Local::now().format("%:z").to_string();
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy().to_string();
                if valid_backup_name(&name) && entry.path().is_file() {
                    names.push(name);
                }
            }
        }
        names.sort();
        names.reverse();
        let mut backups = Vec::new();
        for name in names {
            let bytes = std::fs::metadata(dir.join(&name))
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            backups.push(json!({
                "filename": name,
                "filesize": bytes,
                "timezone": timezone,
            }));
        }
        Ok(json!({ "backups": backups }))
    }

    pub(crate) async fn delete_backups(&self, body: &Value) -> Result<(), AdminFail> {
        let Some(names) = body.get("backups").and_then(Value::as_array) else {
            return Err(bad("Validation failed"));
        };
        let mut files = Vec::new();
        for name in names {
            let Some(name) = name.as_str() else {
                return Err(bad("Validation failed"));
            };
            if !valid_backup_name(name) {
                return Err(bad("Invalid backup name!"));
            }
            files.push(name.to_string());
        }
        for name in files {
            let path = self.backup_dir().join(name);
            if let Err(error) = tokio::fs::remove_file(&path).await {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(DbError::Store(error.to_string()).into());
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn read_backup(
        &self,
        filename: &str,
    ) -> Result<(Vec<u8>, &'static str), AdminFail> {
        if !valid_backup_name(filename) {
            return Err(bad("Invalid backup name!"));
        }
        let path = self.backup_dir().join(filename);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AdminFail::NotFound);
            }
            Err(error) => return Err(DbError::Store(error.to_string()).into()),
        };
        let content_type = if filename.ends_with(".gz") {
            "application/gzip"
        } else {
            "application/sql"
        };
        Ok((bytes, content_type))
    }

    pub(crate) async fn integrity_summary(&self) -> Result<Value, AdminFail> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT type, COUNT(*) FROM integrity_report GROUP BY type")
                .fetch_all(self.pool())
                .await?;
        let mut untracked = 0;
        let mut missing = 0;
        let mut checksum = 0;
        for (kind, count) in rows {
            match kind.as_str() {
                "untracked_file" => untracked = count,
                "missing_file" => missing = count,
                "checksum_mismatch" => checksum = count,
                _ => {}
            }
        }
        Ok(json!({
            "untracked_file": untracked,
            "missing_file": missing,
            "checksum_mismatch": checksum,
        }))
    }

    pub(crate) async fn integrity_report(
        &self,
        kind: &str,
        cursor: Option<&str>,
        limit: i64,
    ) -> Result<Value, AdminFail> {
        if !report_type(kind) || limit <= 0 {
            return Err(bad("Validation failed"));
        }
        let fetch = limit + 1;
        let rows: Vec<(String, String, String)> = if let Some(cursor) = cursor {
            sqlx::query_as(
                "SELECT id, type, path FROM integrity_report
                 WHERE type = ? AND id <= ? ORDER BY id DESC LIMIT ?",
            )
            .bind(kind)
            .bind(cursor)
            .bind(fetch)
            .fetch_all(self.pool())
            .await?
        } else {
            sqlx::query_as(
                "SELECT id, type, path FROM integrity_report
                 WHERE type = ? ORDER BY id DESC LIMIT ?",
            )
            .bind(kind)
            .bind(fetch)
            .fetch_all(self.pool())
            .await?
        };
        let page = limit as usize;
        let mut items = Vec::new();
        for (id, type_name, path) in rows.iter().take(page) {
            items.push(json!({ "id": id, "type": type_name, "path": path }));
        }
        let mut body = json!({ "items": items });
        if let Some((id, _, _)) = rows.get(page) {
            body["nextCursor"] = json!(id);
        }
        Ok(body)
    }

    pub(crate) async fn integrity_csv(&self, kind: &str) -> Result<String, AdminFail> {
        if !report_type(kind) {
            return Err(bad("Validation failed"));
        }
        let rows: Vec<CsvReport> = sqlx::query_as(
            "SELECT id, type AS kind, assetId AS asset_id, fileAssetId AS file_asset_id, path
             FROM integrity_report WHERE type = ? ORDER BY id DESC",
        )
        .bind(kind)
        .fetch_all(self.pool())
        .await?;
        let mut csv = String::from("id,type,assetId,fileAssetId,path\n");
        for row in rows {
            csv.push_str(&csv_field(&row.id));
            csv.push(',');
            csv.push_str(&csv_field(&row.kind));
            csv.push(',');
            csv.push_str(&csv_field(row.asset_id.as_deref().unwrap_or("")));
            csv.push(',');
            csv.push_str(&csv_field(row.file_asset_id.as_deref().unwrap_or("")));
            csv.push(',');
            csv.push_str(&csv_field(&row.path));
            csv.push('\n');
        }
        Ok(csv)
    }

    pub(crate) async fn server_about(&self) -> Result<Value, AdminFail> {
        let licensed = self.system_metadata_value(LICENSE_KEY).await?.is_some();
        Ok(json!({
            "version": "3.3.6",
            "versionUrl": "https://github.com/immich-app/immich/releases/tag/v3.3.6",
            "licensed": licensed,
        }))
    }

    pub(crate) async fn server_statistics(&self) -> Result<Value, AdminFail> {
        let rows: Vec<UserUsage> = sqlx::query_as(
            "SELECT user.id, user.name, user.quotaSizeInBytes AS quota_size_in_bytes,
                COALESCE(SUM(CASE WHEN asset.id IS NOT NULL AND asset.type = 'IMAGE'
                    AND asset.visibility != 'hidden' THEN 1 ELSE 0 END), 0) AS photos,
                COALESCE(SUM(CASE WHEN asset.id IS NOT NULL AND asset.type = 'VIDEO'
                    AND asset.visibility != 'hidden' THEN 1 ELSE 0 END), 0) AS videos,
                COALESCE(SUM(CASE WHEN asset.id IS NOT NULL THEN COALESCE(asset_exif.fileSizeInByte, 0)
                    ELSE 0 END), 0) AS usage,
                COALESCE(SUM(CASE WHEN asset.id IS NOT NULL AND asset.type = 'IMAGE'
                    THEN COALESCE(asset_exif.fileSizeInByte, 0) ELSE 0 END), 0) AS usage_photos,
                COALESCE(SUM(CASE WHEN asset.id IS NOT NULL AND asset.type = 'VIDEO'
                    THEN COALESCE(asset_exif.fileSizeInByte, 0) ELSE 0 END), 0) AS usage_videos
             FROM user
             LEFT JOIN asset ON asset.ownerId = user.id AND asset.deletedAt IS NULL
             LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
             GROUP BY user.id
             ORDER BY user.name ASC",
        )
        .fetch_all(self.pool())
        .await?;
        let mut photos = 0;
        let mut videos = 0;
        let mut usage = 0;
        let mut usage_photos = 0;
        let mut usage_videos = 0;
        let mut usage_by_user = Vec::new();
        for row in rows {
            photos += row.photos;
            videos += row.videos;
            usage += row.usage;
            usage_photos += row.usage_photos;
            usage_videos += row.usage_videos;
            usage_by_user.push(json!({
                "userId": row.id,
                "userName": row.name,
                "photos": row.photos,
                "videos": row.videos,
                "usage": row.usage,
                "usagePhotos": row.usage_photos,
                "usageVideos": row.usage_videos,
                "quotaSizeInBytes": row.quota_size_in_bytes,
            }));
        }
        Ok(json!({
            "photos": photos,
            "videos": videos,
            "usage": usage,
            "usagePhotos": usage_photos,
            "usageVideos": usage_videos,
            "usageByUser": usage_by_user,
        }))
    }

    pub(crate) async fn get_server_license(&self) -> Result<Value, AdminFail> {
        let Some(text) = self.system_metadata_value(LICENSE_KEY).await? else {
            return Err(AdminFail::NotFound);
        };
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub(crate) async fn set_server_license(&self, body: &Value) -> Result<Value, AdminFail> {
        let (key, activation) = license_pair(body)?;
        let (server_key, _) = license_public_keys();
        if !key.starts_with("IMSV-") || !signature_matches(key, activation, server_key) {
            return Err(bad("Invalid license key"));
        }
        let document = license_document(key, activation, &now_iso());
        self.write_system_metadata(LICENSE_KEY, &document.to_string())
            .await?;
        Ok(document)
    }

    pub(crate) async fn delete_server_license(&self) -> Result<(), AdminFail> {
        sqlx::query("DELETE FROM system_metadata WHERE key = ?")
            .bind(LICENSE_KEY)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn get_user_license(&self, user_id: &str) -> Result<Value, AdminFail> {
        let value = self.user_license_value(user_id).await?;
        if value.is_null() {
            Err(AdminFail::NotFound)
        } else {
            Ok(value)
        }
    }

    pub(crate) async fn set_user_license(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<Value, AdminFail> {
        let (key, activation) = license_pair(body)?;
        let (server_key, client_key) = license_public_keys();
        let prefix = key.starts_with("IMCL-") || key.starts_with("IMSV-");
        let signed = signature_matches(key, activation, client_key)
            || signature_matches(key, activation, server_key);
        if !prefix || !signed {
            return Err(bad("Invalid license key"));
        }
        let document = license_document(key, activation, &now_iso());
        let now = now_iso();
        sqlx::query(
            "INSERT INTO user_metadata (userId, key, value, updateId, updatedAt)
             VALUES (?, 'license', ?, ?, ?)
             ON CONFLICT(userId, key) DO UPDATE SET
               value = excluded.value, updateId = excluded.updateId, updatedAt = excluded.updatedAt",
        )
        .bind(user_id)
        .bind(document.to_string())
        .bind(new_id())
        .bind(&now)
        .execute(self.pool())
        .await?;
        Ok(document)
    }

    pub(crate) async fn delete_user_license(&self, user_id: &str) -> Result<(), AdminFail> {
        sqlx::query("DELETE FROM user_metadata WHERE userId = ? AND key = 'license'")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) fn email_template(name: &str, body: &Value) -> Result<Value, AdminFail> {
        if !matches!(
            name,
            "test" | "welcome" | "reset-password" | "album-invite" | "album-update"
        ) {
            return Err(bad("Validation failed"));
        }
        if !body.is_object() && !body.is_null() {
            return Err(bad("Validation failed"));
        }
        let custom = body
            .get("template")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        Ok(json!({
            "name": name,
            "html": template_html(name, custom),
        }))
    }

    async fn queue_document(&self, name: &str) -> Result<Value, AdminFail> {
        let (paused, completed) = self.queue_flags(name).await?;
        let counts = self.job_counts(name).await?;
        Ok(json!({
            "name": name,
            "isPaused": paused,
            "statistics": {
                "active": counts.active,
                "completed": completed,
                "failed": counts.failed,
                "delayed": counts.delayed,
                "waiting": counts.waiting,
                "paused": counts.paused,
            },
        }))
    }

    async fn queue_flags(&self, name: &str) -> Result<(bool, i64), AdminFail> {
        let row: Option<(i64, i64)> =
            sqlx::query_as("SELECT isPaused, completed FROM queue_state WHERE name = ?")
                .bind(name)
                .fetch_optional(self.pool())
                .await?;
        Ok(row
            .map(|(paused, completed)| (paused != 0, completed))
            .unwrap_or((false, 0)))
    }

    async fn job_counts(&self, name: &str) -> Result<JobCounts, AdminFail> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM queue_job WHERE queueName = ? GROUP BY status",
        )
        .bind(name)
        .fetch_all(self.pool())
        .await?;
        let mut counts = JobCounts {
            active: 0,
            failed: 0,
            delayed: 0,
            waiting: 0,
            paused: 0,
        };
        for (status, count) in rows {
            match status.as_str() {
                "active" => counts.active = count,
                "failed" => counts.failed = count,
                "delayed" => counts.delayed = count,
                "waiting" => counts.waiting = count,
                "paused" => counts.paused = count,
                _ => {}
            }
        }
        Ok(counts)
    }

    async fn set_paused(&self, name: &str, paused: bool) -> Result<(), AdminFail> {
        let flag = i64::from(paused);
        sqlx::query(
            "INSERT INTO queue_state (name, isPaused, completed) VALUES (?, ?, 0)
             ON CONFLICT(name) DO UPDATE SET isPaused = excluded.isPaused",
        )
        .bind(name)
        .bind(flag)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn job_rows(
        &self,
        name: &str,
        statuses: &[String],
    ) -> Result<Vec<(String, String, String, i64)>, AdminFail> {
        if statuses.is_empty() {
            let rows = sqlx::query_as(
                "SELECT id, name, data, timestamp FROM queue_job
                 WHERE queueName = ? ORDER BY timestamp DESC",
            )
            .bind(name)
            .fetch_all(self.pool())
            .await?;
            return Ok(rows);
        }
        let placeholders = std::iter::repeat_n("?", statuses.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, name, data, timestamp FROM queue_job
             WHERE queueName = ? AND status IN ({placeholders}) ORDER BY timestamp DESC"
        );
        let mut query = sqlx::query_as::<_, (String, String, String, i64)>(&sql).bind(name);
        for status in statuses {
            query = query.bind(status);
        }
        Ok(query.fetch_all(self.pool()).await?)
    }

    async fn delete_job_statuses(&self, name: &str, statuses: &[&str]) -> Result<(), AdminFail> {
        if statuses.is_empty() {
            return Ok(());
        }
        let placeholders = std::iter::repeat_n("?", statuses.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("DELETE FROM queue_job WHERE queueName = ? AND status IN ({placeholders})");
        let mut query = sqlx::query(&sql).bind(name);
        for status in statuses {
            query = query.bind(status);
        }
        query.execute(self.pool()).await?;
        Ok(())
    }

    async fn start_queue(&self, name: &str) -> Result<(), AdminFail> {
        let counts = self.job_counts(name).await?;
        if counts.active > 0 {
            return Err(bad("Job is already running"));
        }
        match name {
            "videoConversion"
            | "storageTemplateMigration"
            | "migration"
            | "duplicateDetection"
            | "metadataExtraction"
            | "sidecar"
            | "thumbnailGeneration"
            | "library" => Ok(()),
            "smartSearch" => {
                let app = self.clone();
                tokio::spawn(async move {
                    crate::ml::encode_missing_clips(&app).await;
                });
                Ok(())
            }
            "faceDetection" => {
                let app = self.clone();
                tokio::spawn(async move {
                    app.detect_missing_faces().await;
                    app.recognize_unassigned_faces().await;
                });
                Ok(())
            }
            "facialRecognition" => {
                let app = self.clone();
                tokio::spawn(async move {
                    app.recognize_unassigned_faces().await;
                });
                Ok(())
            }
            "ocr" => {
                let app = self.clone();
                tokio::spawn(async move {
                    app.read_missing_ocr().await;
                });
                Ok(())
            }
            "backupDatabase" => self.write_database_backup().await,
            other => Err(AdminFail::BadRequest(format!("Invalid job name: {other}"))),
        }
    }

    async fn person_cleanup(&self) -> Result<(), AdminFail> {
        sqlx::query(
            "DELETE FROM person WHERE NOT EXISTS (
                SELECT 1 FROM asset_face
                WHERE asset_face.personGroupId = person.personGroupId
                  AND asset_face.ownerId = person.ownerId
                  AND asset_face.deletedAt IS NULL
             )",
        )
        .execute(self.pool())
        .await?;
        sqlx::query(
            "DELETE FROM person_group WHERE NOT EXISTS (
                SELECT 1 FROM person WHERE person.personGroupId = person_group.id
             )",
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn tag_cleanup(&self) -> Result<(), AdminFail> {
        sqlx::query(
            "DELETE FROM tag WHERE NOT EXISTS (
                SELECT 1 FROM tag_closure
                JOIN tag_asset ON tag_asset.tagId = tag_closure.id_descendant
                WHERE tag_closure.id_ancestor = tag.id
             )",
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn write_database_backup(&self) -> Result<(), AdminFail> {
        let dir = self.backup_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| AdminFail::from(DbError::Store(error.to_string())))?;
        let filename = format!(
            "immich-db-backup-{}.sql",
            chrono::Utc::now().timestamp_millis()
        );
        let path = dir.join(filename);
        let escaped = path.display().to_string().replace('\'', "''");
        let sql = format!("VACUUM INTO '{escaped}'");
        sqlx::query(&sql).execute(self.pool()).await?;
        Ok(())
    }

    fn backup_dir(&self) -> PathBuf {
        self.data_dir().join("backups")
    }

    fn object_file(&self, key: &str) -> PathBuf {
        self.data_dir().join("objects").join(key)
    }

    async fn scan_missing(&self) -> Result<(), AdminFail> {
        let assets: Vec<(String, String)> =
            sqlx::query_as("SELECT id, originalPath FROM asset WHERE deletedAt IS NULL")
                .fetch_all(self.pool())
                .await?;
        let mut rows = Vec::new();
        for (id, path) in assets {
            if !self.object_file(&path).is_file() {
                rows.push(ReportRow {
                    path,
                    asset_id: Some(id),
                    file_asset_id: None,
                });
            }
        }
        let files: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, assetId, path FROM asset_file WHERE type IN ('thumbnail', 'preview')",
        )
        .fetch_all(self.pool())
        .await?;
        for (file_id, asset_id, path) in files {
            if !self.data_dir().join(&path).is_file() {
                rows.push(ReportRow {
                    path,
                    asset_id: Some(asset_id),
                    file_asset_id: Some(file_id),
                });
            }
        }
        self.replace_reports("missing_file", rows).await
    }

    async fn scan_untracked(&self) -> Result<(), AdminFail> {
        let originals: Vec<String> = sqlx::query_scalar("SELECT originalPath FROM asset")
            .fetch_all(self.pool())
            .await?;
        let files: Vec<String> = sqlx::query_scalar("SELECT path FROM asset_file")
            .fetch_all(self.pool())
            .await?;
        let mut objects = HashSet::new();
        let mut thumbs = HashSet::new();
        for path in originals {
            objects.insert(path);
        }
        for path in files {
            if path.starts_with("thumbs/") {
                thumbs.insert(path);
            } else {
                objects.insert(path);
            }
        }
        let mut found = Vec::new();
        collect_files(&self.data_dir().join("objects"), "", &mut found);
        let mut rows = Vec::new();
        for path in found {
            if !objects.contains(&path) {
                rows.push(ReportRow {
                    path,
                    asset_id: None,
                    file_asset_id: None,
                });
            }
        }
        let mut thumb_files = Vec::new();
        collect_files(&self.data_dir().join("thumbs"), "thumbs", &mut thumb_files);
        for path in thumb_files {
            if !thumbs.contains(&path) {
                rows.push(ReportRow {
                    path,
                    asset_id: None,
                    file_asset_id: None,
                });
            }
        }
        self.replace_reports("untracked_file", rows).await
    }

    async fn scan_checksums(&self) -> Result<(), AdminFail> {
        let assets: Vec<(String, String, Vec<u8>)> =
            sqlx::query_as("SELECT id, originalPath, checksum FROM asset WHERE deletedAt IS NULL")
                .fetch_all(self.pool())
                .await?;
        let mut rows = Vec::new();
        for (id, path, checksum) in assets {
            let file = self.object_file(&path);
            if !file.is_file() {
                continue;
            }
            let Some(hash) = sha1_file(&file) else {
                continue;
            };
            if hash != checksum {
                rows.push(ReportRow {
                    path,
                    asset_id: Some(id),
                    file_asset_id: None,
                });
            }
        }
        self.replace_reports("checksum_mismatch", rows).await
    }

    async fn replace_reports(&self, kind: &str, rows: Vec<ReportRow>) -> Result<(), AdminFail> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("DELETE FROM integrity_report WHERE type = ?")
            .bind(kind)
            .execute(&mut *tx)
            .await?;
        let now = now_iso();
        for row in rows {
            sqlx::query(
                "INSERT INTO integrity_report (id, type, path, assetId, fileAssetId, createdAt)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(new_id())
            .bind(kind)
            .bind(&row.path)
            .bind(&row.asset_id)
            .bind(&row.file_asset_id)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn delete_report(&self, kind: &str) -> Result<(), AdminFail> {
        sqlx::query("DELETE FROM integrity_report WHERE type = ?")
            .bind(kind)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn read_maintenance(&self) -> Result<MaintenanceState, AdminFail> {
        let Some(text) = self.system_metadata_value(MAINTENANCE_KEY).await? else {
            return Ok(MaintenanceState {
                active: false,
                action: "end".to_string(),
                secret: None,
                username: String::new(),
            });
        };
        let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let active = json
            .get("isMaintenanceMode")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !active {
            return Ok(MaintenanceState {
                active: false,
                action: "end".to_string(),
                secret: None,
                username: String::new(),
            });
        }
        Ok(MaintenanceState {
            active: true,
            action: json
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("start")
                .to_string(),
            secret: json
                .get("secret")
                .and_then(Value::as_str)
                .map(str::to_string),
            username: json
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }

    async fn system_metadata_value(&self, key: &str) -> Result<Option<String>, AdminFail> {
        let value = sqlx::query_scalar("SELECT value FROM system_metadata WHERE key = ?")
            .bind(key)
            .fetch_optional(self.pool())
            .await?;
        Ok(value)
    }

    async fn write_system_metadata(&self, key: &str, value: &str) -> Result<(), AdminFail> {
        sqlx::query(
            "INSERT INTO system_metadata (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
