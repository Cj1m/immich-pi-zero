//! On-this-day cards. Generation waits for the nightly run. Upload does not create a card.

use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Utc};
use http::StatusCode;
use serde_json::{json, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};

const STATE_KEY: &str = "memories-state";

pub(crate) enum MemoryError {
    Db(DbError),
    Status(StatusCode, String),
}

impl From<DbError> for MemoryError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for MemoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

pub(crate) async fn nightly_loop(app: App) {
    loop {
        let wait = seconds_until_nightly(&app).await;
        if wait == 0 {
            run_nightly(&app).await;
            tokio::time::sleep(std::time::Duration::from_secs(61)).await;
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(wait.min(60))).await;
        }
    }
}

async fn run_nightly(app: &App) {
    let Ok(config) = app.admin_config().await else {
        return;
    };
    let now = Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    if config["nightlyTasks"]["generateMemories"].as_bool().unwrap_or(true) {
        if let Err(error) = app.generate_memories(&today).await {
            eprintln!("memories: {error:?}");
        }
    }
    if config["nightlyTasks"]["databaseCleanup"].as_bool().unwrap_or(true) {
        let stamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
        if let Err(error) = app.cleanup_memories(&stamp).await {
            eprintln!("memory cleanup: {error:?}");
        }
        if let Err(error) = app.purge_expired_users().await {
            eprintln!("user delete: {error:?}");
        }
    }
}

async fn seconds_until_nightly(app: &App) -> u64 {
    let Ok(config) = app.admin_config().await else {
        return 60;
    };
    let (hour, minute) = start_clock(config["nightlyTasks"]["startTime"].as_str().unwrap_or("00:00"));
    let now = Utc::now();
    let next = next_utc_start(now, hour, minute);
    let seconds = (next - now).num_seconds();
    if seconds <= 0 {
        0
    } else {
        seconds as u64
    }
}

fn start_clock(value: &str) -> (u32, u32) {
    let Some((hour, minute)) = value.split_once(':') else {
        return (0, 0);
    };
    let hour = hour.parse::<u32>().ok().filter(|hour| *hour < 24);
    let minute = minute.parse::<u32>().ok().filter(|minute| *minute < 60);
    match (hour, minute) {
        (Some(hour), Some(minute)) => (hour, minute),
        _ => (0, 0),
    }
}

fn next_utc_start(now: DateTime<Utc>, hour: u32, minute: u32) -> DateTime<Utc> {
    let today = now
        .date_naive()
        .and_hms_opt(hour, minute, 0)
        .unwrap_or_else(|| now.date_naive().and_hms_opt(0, 0, 0).unwrap())
        .and_utc();
    if today >= now {
        today
    } else {
        today + Duration::days(1)
    }
}

impl App {
    /// Fill on-this-day cards for the UTC window around `today` (`YYYY-MM-DD`).
    #[doc(hidden)]
    pub async fn generate_memories(&self, today: &str) -> Result<(), DbError> {
        let today = NaiveDate::parse_from_str(today, "%Y-%m-%d").map_err(|_| DbError::Validation)?;
        let start = today - Duration::days(3);
        let mut last = self.memory_cursor().await?.unwrap_or(start);
        let owners: Vec<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE deletedAt IS NULL")
                .fetch_all(self.pool())
                .await?;
        for offset in 0..=6 {
            let target = start + Duration::days(offset);
            if last >= target {
                continue;
            }
            for owner_id in &owners {
                self.create_on_this_day(owner_id, target).await?;
                self.create_birthday(owner_id, target).await?;
            }
            last = target;
            self.store_memory_cursor(target).await?;
        }
        Ok(())
    }

    /// Drop pairs that left the timeline, and unsaved cards older than 30 days.
    #[doc(hidden)]
    pub async fn cleanup_memories(&self, now: &str) -> Result<(), DbError> {
        let now = DateTime::parse_from_rfc3339(now)
            .map_err(|_| DbError::Validation)?
            .with_timezone(&Utc);
        let cutoff = (now - Duration::days(30)).to_rfc3339_opts(SecondsFormat::Millis, true);
        self.drop_hidden_memory_assets().await?;
        let stale: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, ownerId FROM memory WHERE isSaved = 0 AND createdAt < ?",
        )
        .bind(cutoff)
        .fetch_all(self.pool())
        .await?;
        for (memory_id, owner_id) in stale {
            self.delete_memory_row(&owner_id, &memory_id).await?;
        }
        Ok(())
    }

    pub(crate) async fn search_memories(&self, owner_id: &str, query: &MemoryQuery) -> Result<Vec<Value>, MemoryError> {
        let order = match query.order {
            MemoryOrder::Desc => "showAt IS NULL, showAt DESC, memoryAt DESC",
            MemoryOrder::Asc => "showAt IS NULL, showAt ASC, memoryAt ASC",
            MemoryOrder::Random => "RANDOM()",
        };
        let sql = format!(
            "SELECT id FROM memory
             WHERE ownerId = ?
               AND ((? = 1 AND deletedAt IS NOT NULL) OR (? = 0 AND deletedAt IS NULL))
               AND (? IS NULL OR isSaved = ?)
               AND (? IS NULL OR type = ?)
               AND (? IS NULL OR id = ?)
               AND (? IS NULL OR showAt IS NULL OR showAt <= ?)
               AND (? IS NULL OR hideAt IS NULL OR hideAt >= ?)
               AND (? IS NULL OR (? = 1 AND showAt > ?) OR (? = 0 AND (showAt IS NULL OR showAt <= ?)))
             ORDER BY {order}
             LIMIT ? OFFSET ?"
        );
        let ids: Vec<String> = bind_filters(sqlx::query_scalar(&sql), owner_id, query)
            .bind(query.limit)
            .bind(query.offset)
            .fetch_all(self.pool())
            .await?;
        let mut memories = Vec::new();
        for id in ids {
            let Some(document) = self.memory_document(owner_id, &id).await? else {
                continue;
            };
            if document["assets"].as_array().is_some_and(|assets| !assets.is_empty()) {
                memories.push(document);
            }
        }
        Ok(memories)
    }

    pub(crate) async fn memory_statistics(&self, owner_id: &str, query: &MemoryQuery) -> Result<i64, MemoryError> {
        let sql = "SELECT COUNT(*) FROM memory
             WHERE ownerId = ?
               AND ((? = 1 AND deletedAt IS NOT NULL) OR (? = 0 AND deletedAt IS NULL))
               AND (? IS NULL OR isSaved = ?)
               AND (? IS NULL OR type = ?)
               AND (? IS NULL OR id = ?)
               AND (? IS NULL OR showAt IS NULL OR showAt <= ?)
               AND (? IS NULL OR hideAt IS NULL OR hideAt >= ?)
               AND (? IS NULL OR (? = 1 AND showAt > ?) OR (? = 0 AND (showAt IS NULL OR showAt <= ?)))";
        let total: i64 = bind_filters(sqlx::query_scalar(sql), owner_id, query)
            .fetch_one(self.pool())
            .await?;
        Ok(total)
    }

    pub(crate) async fn update_memory(&self, owner_id: &str, id: &str, body: &Value) -> Result<Value, MemoryError> {
        if !body.is_object() || body.as_object().is_some_and(|object| object.is_empty()) {
            return Err(MemoryError::Status(StatusCode::BAD_REQUEST, "Validation failed".to_string()));
        }
        let is_saved = match body.get("isSaved") {
            None => None,
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => {
                return Err(MemoryError::Status(StatusCode::BAD_REQUEST, "Validation failed".to_string()));
            }
        };
        let seen_at = optional_time(body, "seenAt")?;
        let memory_at = optional_time(body, "memoryAt")?;
        if is_saved.is_none() && seen_at.is_none() && memory_at.is_none() {
            return Err(MemoryError::Status(StatusCode::BAD_REQUEST, "Validation failed".to_string()));
        }
        if self.memory_owner(id).await? != Some(owner_id.to_string()) {
            return Err(MemoryError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no memory.update access".to_string(),
            ));
        }
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "UPDATE memory SET
                isSaved = COALESCE(?, isSaved),
                seenAt = COALESCE(?, seenAt),
                memoryAt = COALESCE(?, memoryAt),
                updatedAt = ?,
                updateId = ?
             WHERE id = ?",
        )
        .bind(is_saved.map(i64::from))
        .bind(seen_at)
        .bind(memory_at)
        .bind(&now)
        .bind(&update_id)
        .bind(id)
        .execute(self.pool())
        .await?;
        self.memory_document(owner_id, id)
            .await?
            .ok_or_else(|| MemoryError::Status(StatusCode::BAD_REQUEST, "Not found or no memory.update access".to_string()))
    }

    pub(crate) async fn delete_memory(&self, owner_id: &str, id: &str) -> Result<(), MemoryError> {
        if self.memory_owner(id).await? != Some(owner_id.to_string()) {
            return Err(MemoryError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no memory.delete access".to_string(),
            ));
        }
        self.delete_memory_row(owner_id, id).await?;
        Ok(())
    }

    pub(crate) async fn remove_memory_assets(
        &self,
        owner_id: &str,
        id: &str,
        ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if self.memory_owner(id).await? != Some(owner_id.to_string()) {
            return Err(MemoryError::Status(
                StatusCode::BAD_REQUEST,
                "Not found or no memory.update access".to_string(),
            ));
        }
        let existing: Vec<String> = sqlx::query_scalar("SELECT assetId FROM memory_asset WHERE memoryId = ?")
            .bind(id)
            .fetch_all(self.pool())
            .await?;
        let mut results = Vec::new();
        let mut removed = false;
        for asset_id in ids {
            if !existing.iter().any(|item| item == asset_id) {
                results.push(json!({ "id": asset_id, "success": false, "error": "not_found" }));
                continue;
            }
            self.audit_memory_asset(owner_id, id, asset_id).await?;
            sqlx::query("DELETE FROM memory_asset WHERE memoryId = ? AND assetId = ?")
                .bind(id)
                .bind(asset_id)
                .execute(self.pool())
                .await?;
            results.push(json!({ "id": asset_id, "success": true }));
            removed = true;
        }
        if removed {
            sqlx::query("UPDATE memory SET updatedAt = ?, updateId = ? WHERE id = ?")
                .bind(now_iso())
                .bind(new_update_id())
                .bind(id)
                .execute(self.pool())
                .await?;
        }
        Ok(results)
    }

    async fn create_on_this_day(&self, owner_id: &str, target: NaiveDate) -> Result<(), DbError> {
        let month_day = format!("{:02}-{:02}", target.month(), target.day());
        let rows: Vec<(String, String, i32)> = sqlx::query_as(
            "SELECT id, localDateTime, CAST(substr(localDateTime, 1, 4) AS INTEGER)
             FROM asset
             WHERE ownerId = ?
               AND deletedAt IS NULL
               AND visibility = 'timeline'
               AND substr(localDateTime, 6, 5) = ?
               AND CAST(substr(localDateTime, 1, 4) AS INTEGER) < ?
               AND EXISTS (
                   SELECT 1 FROM asset_file WHERE assetId = asset.id AND type = 'preview'
               )
             ORDER BY localDateTime DESC",
        )
        .bind(owner_id)
        .bind(&month_day)
        .bind(target.year())
        .fetch_all(self.pool())
        .await?;
        let mut by_year: std::collections::BTreeMap<i32, Vec<String>> = std::collections::BTreeMap::new();
        for (asset_id, _local, year) in rows {
            let assets = by_year.entry(year).or_default();
            if assets.len() < 20 {
                assets.push(asset_id);
            }
        }
        let show_at = day_start(target);
        let hide_at = day_end(target);
        for (year, asset_ids) in by_year.into_iter().rev() {
            let memory_at = day_start(NaiveDate::from_ymd_opt(year, target.month(), target.day()).unwrap_or(target));
            let exists: Option<String> = sqlx::query_scalar(
                "SELECT id FROM memory
                 WHERE ownerId = ? AND type = 'on_this_day' AND memoryAt = ? AND showAt = ?",
            )
            .bind(owner_id)
            .bind(&memory_at)
            .bind(&show_at)
            .fetch_optional(self.pool())
            .await?;
            if exists.is_some() {
                continue;
            }
            let data = json!({ "year": year }).to_string();
            self.insert_memory(NewMemory {
                owner_id,
                kind: "on_this_day",
                data: &data,
                memory_at: &memory_at,
                show_at: &show_at,
                hide_at: &hide_at,
                asset_ids: &asset_ids,
            })
            .await?;
        }
        Ok(())
    }

    async fn create_birthday(&self, _owner_id: &str, _target: NaiveDate) -> Result<(), DbError> {
        let table: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'person'",
        )
        .fetch_optional(self.pool())
        .await?;
        if table.is_none() {
            return Ok(());
        }
        Ok(())
    }

    async fn insert_memory(&self, memory: NewMemory<'_>) -> Result<(), DbError> {
        let id = new_id();
        let now = now_iso();
        let mut transaction = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO memory
                (id, createdAt, updatedAt, deletedAt, ownerId, type, data, isSaved, memoryAt, seenAt, showAt, hideAt, updateId)
             VALUES (?, ?, ?, NULL, ?, ?, ?, 0, ?, NULL, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&now)
        .bind(&now)
        .bind(memory.owner_id)
        .bind(memory.kind)
        .bind(memory.data)
        .bind(memory.memory_at)
        .bind(memory.show_at)
        .bind(memory.hide_at)
        .bind(new_update_id())
        .execute(&mut *transaction)
        .await?;
        for asset_id in memory.asset_ids {
            sqlx::query("INSERT INTO memory_asset (memoryId, assetId, updateId) VALUES (?, ?, ?)")
                .bind(&id)
                .bind(asset_id)
                .bind(new_update_id())
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn memory_document(&self, owner_id: &str, id: &str) -> Result<Option<Value>, MemoryError> {
        let row: Option<MemoryRow> = sqlx::query_as(
            "SELECT id, createdAt AS created_at, updatedAt AS updated_at, deletedAt AS deleted_at,
                    ownerId AS owner_id, type AS kind, data, isSaved AS is_saved, memoryAt AS memory_at,
                    seenAt AS seen_at, showAt AS show_at, hideAt AS hide_at
             FROM memory WHERE id = ? AND ownerId = ?",
        )
        .bind(id)
        .bind(owner_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let asset_ids: Vec<String> = sqlx::query_scalar(
            "SELECT asset.id FROM asset
             JOIN memory_asset ON memory_asset.assetId = asset.id
             WHERE memory_asset.memoryId = ?
               AND asset.deletedAt IS NULL
               AND asset.visibility = 'timeline'
             ORDER BY asset.fileCreatedAt ASC, asset.id ASC",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        let mut assets = Vec::new();
        for asset_id in asset_ids {
            assets.push(self.asset_json(owner_id, &asset_id).await?);
        }
        let data = serde_json::from_str::<Value>(&row.data).unwrap_or_else(|_| json!({}));
        Ok(Some(json!({
            "id": row.id,
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "deletedAt": row.deleted_at,
            "memoryAt": row.memory_at,
            "seenAt": row.seen_at,
            "showAt": row.show_at,
            "hideAt": row.hide_at,
            "ownerId": row.owner_id,
            "type": row.kind,
            "data": data,
            "isSaved": row.is_saved != 0,
            "assets": assets,
        })))
    }

    async fn memory_owner(&self, id: &str) -> Result<Option<String>, MemoryError> {
        let owner: Option<String> = sqlx::query_scalar("SELECT ownerId FROM memory WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(owner)
    }

    async fn delete_memory_row(&self, owner_id: &str, id: &str) -> Result<(), DbError> {
        let assets: Vec<String> = sqlx::query_scalar("SELECT assetId FROM memory_asset WHERE memoryId = ?")
            .bind(id)
            .fetch_all(self.pool())
            .await?;
        for asset_id in assets {
            self.audit_memory_asset(owner_id, id, &asset_id).await?;
        }
        sqlx::query("INSERT INTO memory_audit (id, memoryId, ownerId, updateId) VALUES (?, ?, ?, ?)")
            .bind(new_id())
            .bind(id)
            .bind(owner_id)
            .bind(new_update_id())
            .execute(self.pool())
            .await?;
        sqlx::query("DELETE FROM memory WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn audit_memory_asset(&self, owner_id: &str, memory_id: &str, asset_id: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO memory_asset_audit (id, memoryId, assetId, ownerId, updateId) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(new_id())
        .bind(memory_id)
        .bind(asset_id)
        .bind(owner_id)
        .bind(new_update_id())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn drop_hidden_memory_assets(&self) -> Result<(), DbError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT memory.ownerId, memory_asset.memoryId, memory_asset.assetId
             FROM memory_asset
             JOIN memory ON memory.id = memory_asset.memoryId
             JOIN asset ON asset.id = memory_asset.assetId
             WHERE asset.visibility != 'timeline'",
        )
        .fetch_all(self.pool())
        .await?;
        for (owner_id, memory_id, asset_id) in rows {
            self.audit_memory_asset(&owner_id, &memory_id, &asset_id).await?;
            sqlx::query("DELETE FROM memory_asset WHERE memoryId = ? AND assetId = ?")
                .bind(&memory_id)
                .bind(&asset_id)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    async fn memory_cursor(&self) -> Result<Option<NaiveDate>, DbError> {
        let value: Option<String> = sqlx::query_scalar("SELECT value FROM system_metadata WHERE key = ?")
            .bind(STATE_KEY)
            .fetch_optional(self.pool())
            .await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let parsed: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
        let Some(text) = parsed["lastOnThisDayDate"].as_str() else {
            return Ok(None);
        };
        Ok(NaiveDate::parse_from_str(&text[..text.len().min(10)], "%Y-%m-%d").ok())
    }

    async fn store_memory_cursor(&self, day: NaiveDate) -> Result<(), DbError> {
        let value = json!({ "lastOnThisDayDate": day_start(day) }).to_string();
        sqlx::query(
            "INSERT INTO system_metadata (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(STATE_KEY)
        .bind(value)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

pub(crate) enum MemoryOrder {
    Asc,
    Desc,
    Random,
}

struct NewMemory<'a> {
    owner_id: &'a str,
    kind: &'a str,
    data: &'a str,
    memory_at: &'a str,
    show_at: &'a str,
    hide_at: &'a str,
    asset_ids: &'a [String],
}

pub(crate) struct MemoryQuery {
    pub trashed: bool,
    pub saved: Option<i64>,
    pub kind: Option<String>,
    pub id: Option<String>,
    pub for_instant: Option<String>,
    pub upcoming: Option<bool>,
    pub now: String,
    pub order: MemoryOrder,
    pub limit: i64,
    pub offset: i64,
}

fn bind_filters<'q, O>(
    query: sqlx::query::QueryScalar<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>>,
    owner_id: &'q str,
    filters: &'q MemoryQuery,
) -> sqlx::query::QueryScalar<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>> {
    let upcoming_flag = filters.upcoming.map(i64::from);
    query
        .bind(owner_id)
        .bind(i64::from(filters.trashed))
        .bind(i64::from(filters.trashed))
        .bind(filters.saved)
        .bind(filters.saved)
        .bind(filters.kind.as_deref())
        .bind(filters.kind.as_deref())
        .bind(filters.id.as_deref())
        .bind(filters.id.as_deref())
        .bind(filters.for_instant.as_deref())
        .bind(filters.for_instant.as_deref())
        .bind(filters.for_instant.as_deref())
        .bind(filters.for_instant.as_deref())
        .bind(upcoming_flag)
        .bind(upcoming_flag)
        .bind(&filters.now)
        .bind(upcoming_flag)
        .bind(&filters.now)
}

fn optional_time(body: &Value, key: &str) -> Result<Option<String>, MemoryError> {
    match body.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => match DateTime::parse_from_rfc3339(value) {
            Ok(time) => Ok(Some(time.to_rfc3339_opts(SecondsFormat::Millis, true))),
            Err(_) => Err(MemoryError::Status(StatusCode::BAD_REQUEST, "Validation failed".to_string())),
        },
        Some(_) => Err(MemoryError::Status(StatusCode::BAD_REQUEST, "Validation failed".to_string())),
    }
}

fn day_start(day: NaiveDate) -> String {
    format!("{day}T00:00:00.000Z")
}

fn day_end(day: NaiveDate) -> String {
    format!("{day}T23:59:59.999Z")
}

#[derive(sqlx::FromRow)]
struct MemoryRow {
    id: String,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    owner_id: String,
    kind: String,
    data: String,
    is_saved: i64,
    memory_at: String,
    seen_at: Option<String>,
    show_at: Option<String>,
    hide_at: Option<String>,
}
