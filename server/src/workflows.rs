//! Workflow documents and the asset runs the editor can see. The phone does not sync them.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::db::{new_id, new_update_id, now_iso, App, DbError};
use crate::media::checksum_base64;
use crate::plugins::{self, PluginCall};

const TRIGGERS: [&str; 3] = ["AssetCreate", "AssetMetadataExtraction", "AssetTagged"];

pub(crate) enum WorkflowFail {
    Db(DbError),
    BadRequest(String),
}

impl From<DbError> for WorkflowFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for WorkflowFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

#[derive(sqlx::FromRow)]
struct WorkflowRow {
    id: String,
    trigger: String,
    name: Option<String>,
    description: Option<String>,
    created_at: String,
    updated_at: String,
    enabled: i64,
    logging: i64,
}

#[derive(sqlx::FromRow)]
struct StepRow {
    plugin_name: String,
    method_name: String,
    config: Option<String>,
    enabled: i64,
}

struct ResolvedStep {
    method_id: String,
    config: Option<String>,
    enabled: bool,
}

impl App {
    pub(crate) fn workflow_triggers(&self) -> Value {
        Value::Array(
            TRIGGERS
                .into_iter()
                .map(|trigger| json!({"trigger": trigger, "types": ["AssetV1"]}))
                .collect(),
        )
    }

    pub(crate) async fn list_workflows(
        &self,
        user_id: &str,
        query: &BTreeMap<String, String>,
    ) -> Result<Value, WorkflowFail> {
        let mut sql = String::from(
            "SELECT id, trigger, name, description, createdAt AS created_at,
                    updatedAt AS updated_at, enabled, logging
             FROM workflow WHERE ownerId = ?",
        );
        let mut binds: Vec<String> = vec![user_id.to_string()];
        if let Some(id) = query.get("id") {
            require_uuid(id)?;
            sql.push_str(" AND id = ?");
            binds.push(id.clone());
        }
        if let Some(trigger) = query.get("trigger") {
            require_trigger(trigger)?;
            sql.push_str(" AND trigger = ?");
            binds.push(trigger.clone());
        }
        for column in ["name", "description"] {
            if let Some(value) = query.get(column) {
                sql.push_str(&format!(" AND {column} = ?"));
                binds.push(value.clone());
            }
        }
        if let Some(enabled) = query.get("enabled") {
            sql.push_str(" AND enabled = ?");
            binds.push(i64_bool(enabled)?.to_string());
        }
        if let Some(logging) = query.get("logging") {
            sql.push_str(" AND logging = ?");
            binds.push(i64_bool(logging)?.to_string());
        }
        sql.push_str(" ORDER BY createdAt DESC");
        let mut statement = sqlx::query_as::<_, WorkflowRow>(&sql);
        for bind in &binds {
            statement = statement.bind(bind);
        }
        let rows = statement.fetch_all(self.pool()).await?;
        let mut workflows = Vec::new();
        for row in rows {
            workflows.push(self.workflow_document(&row, false).await?);
        }
        Ok(Value::Array(workflows))
    }

    pub(crate) async fn create_workflow(
        &self,
        user_id: &str,
        body: &Value,
    ) -> Result<Value, WorkflowFail> {
        let trigger = body
            .get("trigger")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkflowFail::BadRequest("Validation failed".into()))?;
        require_trigger(trigger)?;
        let name = optional_text(body, "name")?;
        let description = optional_text(body, "description")?;
        let enabled = optional_bool(body, "enabled")?.unwrap_or(true);
        let logging = optional_bool(body, "logging")?.unwrap_or(false);
        let steps = if body.get("steps").is_some() {
            self.resolve_steps(body.get("steps"), trigger).await?
        } else {
            Vec::new()
        };
        let id = new_id();
        let now = now_iso();
        let update_id = new_update_id();
        sqlx::query(
            "INSERT INTO workflow (
                id, ownerId, trigger, name, description, createdAt, updatedAt, updateId, enabled, logging
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(trigger)
        .bind(&name)
        .bind(&description)
        .bind(&now)
        .bind(&now)
        .bind(&update_id)
        .bind(i64::from(enabled))
        .bind(i64::from(logging))
        .execute(self.pool())
        .await?;
        self.replace_steps(&id, &steps).await?;
        Ok(json!({
            "id": id,
            "trigger": trigger,
            "name": name,
            "description": description,
            "createdAt": now,
            "updatedAt": now,
            "enabled": enabled,
            "logging": logging,
            "steps": [],
        }))
    }

    pub(crate) async fn get_workflow(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<Value, WorkflowFail> {
        self.require_workflow(user_id, id, "workflow.read").await?;
        let row = self.workflow_row(id).await?;
        self.workflow_document(&row, false).await
    }

    pub(crate) async fn share_workflow(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<Value, WorkflowFail> {
        self.require_workflow(user_id, id, "workflow.read").await?;
        let row = self.workflow_row(id).await?;
        self.workflow_document(&row, true).await
    }

    pub(crate) async fn update_workflow(
        &self,
        user_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, WorkflowFail> {
        self.require_workflow(user_id, id, "workflow.update")
            .await?;
        let mut row = self.workflow_row(id).await?;
        let mut changed = false;
        if let Some(trigger) = body.get("trigger") {
            let trigger = trigger
                .as_str()
                .ok_or_else(|| WorkflowFail::BadRequest("Validation failed".into()))?;
            require_trigger(trigger)?;
            row.trigger = trigger.to_string();
            changed = true;
        }
        if body.get("name").is_some() {
            row.name = optional_text(body, "name")?;
            changed = true;
        }
        if body.get("description").is_some() {
            row.description = optional_text(body, "description")?;
            changed = true;
        }
        if let Some(enabled) = optional_bool(body, "enabled")? {
            row.enabled = i64::from(enabled);
            changed = true;
        }
        if let Some(logging) = optional_bool(body, "logging")? {
            if !logging {
                sqlx::query("DELETE FROM workflow_log WHERE workflowId = ?")
                    .bind(id)
                    .execute(self.pool())
                    .await?;
            }
            row.logging = i64::from(logging);
            changed = true;
        }
        let steps = if body.get("steps").is_some() {
            Some(self.resolve_steps(body.get("steps"), &row.trigger).await?)
        } else {
            None
        };
        if changed {
            let now = now_iso();
            row.updated_at = now.clone();
            sqlx::query(
                "UPDATE workflow
                 SET trigger = ?, name = ?, description = ?, enabled = ?, logging = ?,
                     updatedAt = ?, updateId = ?
                 WHERE id = ?",
            )
            .bind(&row.trigger)
            .bind(&row.name)
            .bind(&row.description)
            .bind(row.enabled)
            .bind(row.logging)
            .bind(&now)
            .bind(new_update_id())
            .bind(id)
            .execute(self.pool())
            .await?;
        }
        if let Some(steps) = steps {
            self.replace_steps(id, &steps).await?;
        }
        let row = self.workflow_row(id).await?;
        self.workflow_document(&row, false).await
    }

    pub(crate) async fn delete_workflow(
        &self,
        user_id: &str,
        id: &str,
    ) -> Result<(), WorkflowFail> {
        self.require_workflow(user_id, id, "workflow.delete")
            .await?;
        sqlx::query("DELETE FROM workflow WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn workflow_logs(
        &self,
        user_id: &str,
        id: &str,
        query: &BTreeMap<String, String>,
    ) -> Result<Value, WorkflowFail> {
        self.require_workflow(user_id, id, "workflow.logs").await?;
        let limit = match query.get("limit") {
            None => 50_i64,
            Some(value) => value
                .parse::<i64>()
                .ok()
                .filter(|limit| *limit > 0)
                .ok_or_else(|| WorkflowFail::BadRequest("Validation failed".into()))?,
        };
        let mut sql = String::from(
            "SELECT workflow_log.id AS id, workflow_log.createdAt AS created_at,
                    workflow_log.result AS result, workflow_log.triggerDataId AS trigger_data_id,
                    plugin_method.pluginId AS plugin_id, plugin_method.name AS method_name,
                    workflow_step.\"order\" AS step_order
             FROM workflow_log
             LEFT JOIN workflow_step ON workflow_step.id = workflow_log.workflowStepId
             LEFT JOIN plugin_method ON plugin_method.id = workflow_step.pluginMethodId
             WHERE workflow_log.workflowId = ?",
        );
        let mut binds = vec![id.to_string()];
        if let Some(result) = query.get("result") {
            if !matches!(result.as_str(), "completed" | "halted" | "error") {
                return Err(WorkflowFail::BadRequest("Validation failed".into()));
            }
            sql.push_str(" AND workflow_log.result = ?");
            binds.push(result.clone());
        }
        if let Some(before) = query.get("before") {
            if chrono::DateTime::parse_from_rfc3339(before).is_err() {
                return Err(WorkflowFail::BadRequest("Validation failed".into()));
            }
            sql.push_str(" AND workflow_log.createdAt < ?");
            binds.push(before.clone());
        }
        sql.push_str(" ORDER BY workflow_log.createdAt DESC LIMIT ?");
        let mut statement = sqlx::query_as::<_, LogRow>(&sql);
        for bind in &binds {
            statement = statement.bind(bind);
        }
        statement = statement.bind(limit);
        let rows = statement.fetch_all(self.pool()).await?;
        Ok(Value::Array(rows.iter().map(log_json).collect()))
    }

    pub(crate) async fn run_asset_workflows(&self, user_id: &str, asset_id: &str, trigger: &str) {
        self.run_asset_workflows_at(user_id, asset_id, trigger, 0)
            .await;
    }

    pub(crate) async fn run_asset_workflows_at(
        &self,
        user_id: &str,
        asset_id: &str,
        trigger: &str,
        depth: u32,
    ) {
        if depth >= 8 {
            eprintln!("workflow: nested too deep");
            return;
        }
        if let Err(error) = self
            .execute_workflows(user_id, asset_id, trigger, depth)
            .await
        {
            eprintln!("workflow: {error}");
        }
    }

    async fn execute_workflows(
        &self,
        user_id: &str,
        asset_id: &str,
        trigger: &str,
        depth: u32,
    ) -> Result<(), String> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM workflow
             WHERE ownerId = ? AND trigger = ? AND enabled = 1
             ORDER BY createdAt DESC",
        )
        .bind(user_id)
        .bind(trigger)
        .fetch_all(self.pool())
        .await
        .map_err(|error| error.to_string())?;
        for id in ids {
            if let Err(error) = self.execute_workflow(&id, asset_id, depth).await {
                eprintln!("workflow: {error}");
            }
        }
        Ok(())
    }

    async fn execute_workflow(
        &self,
        workflow_id: &str,
        asset_id: &str,
        depth: u32,
    ) -> Result<(), String> {
        let header = sqlx::query_as::<_, (String, i64)>(
            "SELECT trigger, logging FROM workflow WHERE id = ? AND enabled = 1",
        )
        .bind(workflow_id)
        .fetch_optional(self.pool())
        .await
        .map_err(|error| error.to_string())?;
        let Some((trigger, logging)) = header else {
            return Ok(());
        };
        let steps = sqlx::query_as::<_, RunStep>(
            "SELECT workflow_step.id AS id, workflow_step.config AS config,
                    workflow_step.\"order\" AS step_order, plugin_method.name AS method_name,
                    plugin_method.types AS types, plugin_method.hostFunctions AS host_functions,
                    plugin_method.allowedHosts AS allowed_hosts, plugin.id AS plugin_id,
                    plugin.enabled AS plugin_enabled, plugin.wasmBytes AS wasm_bytes
             FROM workflow_step
             JOIN plugin_method ON plugin_method.id = workflow_step.pluginMethodId
             JOIN plugin ON plugin.id = plugin_method.pluginId
             WHERE workflow_step.workflowId = ? AND workflow_step.enabled = 1
             ORDER BY workflow_step.\"order\"",
        )
        .bind(workflow_id)
        .fetch_all(self.pool())
        .await
        .map_err(|error| error.to_string())?;
        if steps.iter().any(|step| !step.types.contains("AssetV1")) {
            return Err("Unable to infer workflow event type from steps".into());
        }
        let run_id = new_id();
        let logging = logging != 0;
        for step in &steps {
            if step.method_name.starts_with("noop") {
                continue;
            }
            let outcome = self
                .run_step(&trigger, workflow_id, asset_id, step, depth)
                .await;
            match outcome {
                StepOutcome::Continue => {}
                StepOutcome::Halt => {
                    self.write_log(
                        logging,
                        workflow_id,
                        "halted",
                        Some(&step.id),
                        asset_id,
                        &run_id,
                    )
                    .await;
                    return Ok(());
                }
                StepOutcome::Error(error) => {
                    eprintln!("workflow: {workflow_id} step {}: {error}", step.id);
                    self.write_log(
                        logging,
                        workflow_id,
                        "error",
                        Some(&step.id),
                        asset_id,
                        &run_id,
                    )
                    .await;
                    return Ok(());
                }
            }
        }
        self.write_log(logging, workflow_id, "completed", None, asset_id, &run_id)
            .await;
        Ok(())
    }

    async fn run_step(
        &self,
        trigger: &str,
        workflow_id: &str,
        asset_id: &str,
        step: &RunStep,
        depth: u32,
    ) -> StepOutcome {
        if step.plugin_enabled == 0 {
            return StepOutcome::Error("plugin is disabled".into());
        }
        let asset = match self.workflow_asset(asset_id).await {
            Ok(asset) => asset,
            Err(error) => return StepOutcome::Error(error),
        };
        let config = step
            .config
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| json!({}));
        let owner_id = asset.owner_id.clone();
        let input = json!({
            "trigger": trigger,
            "type": "AssetV1",
            "config": config,
            "workflow": {
                "id": workflow_id,
                "authToken": &owner_id,
                "stepId": step.id,
            },
            "data": {"asset": asset.value},
        });
        let allowed_hosts =
            serde_json::from_str::<Vec<String>>(&step.allowed_hosts).unwrap_or_default();
        let called = plugins::call_plugin(
            self,
            PluginCall {
                wasm: step.wasm_bytes.clone(),
                method: step.method_name.clone(),
                input: input.to_string(),
                allow_host: step.host_functions != 0,
                user_id: owner_id.clone(),
                allowed_hosts,
                depth,
            },
        )
        .await;
        let output = match called {
            Ok(output) => output,
            Err(error) => return StepOutcome::Error(error),
        };
        let result = output
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| json!({}));
        if let Some(changes) = result.get("changes") {
            if let Err(error) = self
                .apply_workflow_changes(&owner_id, asset_id, changes)
                .await
            {
                return StepOutcome::Error(format!("{error:?}"));
            }
        }
        if let Some(config) = result.get("config").filter(|value| value.is_object()) {
            if let Err(error) = sqlx::query("UPDATE workflow_step SET config = ? WHERE id = ?")
                .bind(config.to_string())
                .bind(&step.id)
                .execute(self.pool())
                .await
            {
                return StepOutcome::Error(error.to_string());
            }
        }
        let proceed = result
            .pointer("/workflow/continue")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if proceed {
            StepOutcome::Continue
        } else {
            StepOutcome::Halt
        }
    }

    async fn apply_workflow_changes(
        &self,
        owner_id: &str,
        asset_id: &str,
        changes: &Value,
    ) -> Result<(), DbError> {
        let Some(asset) = changes.get("asset").filter(|value| value.is_object()) else {
            return Ok(());
        };
        let mut body = Map::new();
        if let Some(favorite) = asset.get("isFavorite").filter(|value| value.is_boolean()) {
            body.insert("isFavorite".to_string(), favorite.clone());
        }
        if let Some(visibility) = asset.get("visibility").filter(|value| value.is_string()) {
            body.insert("visibility".to_string(), visibility.clone());
        }
        if let Some(exif) = asset.get("exifInfo").filter(|value| value.is_object()) {
            if let Some(description) = exif.get("description").filter(|value| value.is_string()) {
                body.insert("description".to_string(), description.clone());
            }
            if let Some(date) = exif
                .get("dateTimeOriginal")
                .filter(|value| value.is_string())
            {
                body.insert("dateTimeOriginal".to_string(), date.clone());
            }
            if let Some(rating) = exif.get("rating") {
                if rating.is_null() || rating.is_number() {
                    body.insert("rating".to_string(), rating.clone());
                }
            }
            let latitude = exif.get("latitude").filter(|value| value.is_number());
            let longitude = exif.get("longitude").filter(|value| value.is_number());
            if let (Some(latitude), Some(longitude)) = (latitude, longitude) {
                body.insert("latitude".to_string(), latitude.clone());
                body.insert("longitude".to_string(), longitude.clone());
            }
        }
        if body.is_empty() {
            return Ok(());
        }
        self.update_asset(owner_id, asset_id, &Value::Object(body))
            .await?;
        Ok(())
    }

    async fn workflow_asset(&self, asset_id: &str) -> Result<WorkflowAsset, String> {
        let owner_id: Option<String> =
            sqlx::query_scalar("SELECT ownerId FROM asset WHERE id = ? AND deletedAt IS NULL")
                .bind(asset_id)
                .fetch_optional(self.pool())
                .await
                .map_err(|error| error.to_string())?;
        let Some(owner_id) = owner_id else {
            return Err(format!("asset {asset_id} was not found"));
        };
        let record = self
            .asset_record(&owner_id, asset_id)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let tags = self
            .asset_tag_documents(asset_id)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let exif = json!({
            "make": record.make,
            "model": record.model,
            "orientation": record.orientation,
            "dateTimeOriginal": record.date_time_original,
            "modifyDate": record.modify_date,
            "exifImageWidth": record.exif_width.or(record.width),
            "exifImageHeight": record.exif_height.or(record.height),
            "fileSizeInByte": record.file_size,
            "lensModel": record.lens_model,
            "fNumber": record.f_number,
            "focalLength": record.focal_length,
            "iso": record.iso,
            "latitude": record.latitude,
            "longitude": record.longitude,
            "city": record.city,
            "state": record.state,
            "country": record.country,
            "description": record.description,
            "exposureTime": record.exposure_time,
            "timeZone": record.time_zone,
            "projectionType": record.projection_type,
            "rating": record.rating,
            "updatedAt": record.updated_at,
        });
        let value = json!({
            "id": record.id,
            "ownerId": record.owner_id,
            "stackId": record.stack_id,
            "livePhotoVideoId": record.live_photo_video_id,
            "libraryId": null,
            "duplicateId": record.duplicate_id,
            "createdAt": record.created_at,
            "updatedAt": record.updated_at,
            "deletedAt": record.deleted_at,
            "fileCreatedAt": record.file_created_at,
            "fileModifiedAt": record.file_modified_at,
            "localDateTime": record.local_date_time,
            "type": record.asset_type,
            "visibility": record.visibility,
            "duration": record.duration,
            "checksum": checksum_base64(&record.checksum),
            "originalPath": record.original_path,
            "originalFileName": record.original_file_name,
            "isOffline": record.is_offline != 0,
            "isFavorite": record.is_favorite != 0,
            "isExternal": false,
            "isEdited": record.is_edited != 0,
            "tags": tags,
            "exifInfo": exif,
        });
        Ok(WorkflowAsset { owner_id, value })
    }

    async fn write_log(
        &self,
        logging: bool,
        workflow_id: &str,
        result: &str,
        step_id: Option<&str>,
        asset_id: &str,
        run_id: &str,
    ) {
        if !logging {
            return;
        }
        if let Err(error) = sqlx::query(
            "INSERT INTO workflow_log (
                id, createdAt, workflowId, result, workflowStepId, triggerDataId, runId
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(new_id())
        .bind(now_iso())
        .bind(workflow_id)
        .bind(result)
        .bind(step_id)
        .bind(asset_id)
        .bind(run_id)
        .execute(self.pool())
        .await
        {
            eprintln!("workflow log: {error}");
        }
    }

    async fn require_workflow(
        &self,
        user_id: &str,
        id: &str,
        permission: &str,
    ) -> Result<(), WorkflowFail> {
        let owned: Option<String> =
            sqlx::query_scalar("SELECT id FROM workflow WHERE id = ? AND ownerId = ?")
                .bind(id)
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        if owned.is_none() {
            return Err(WorkflowFail::BadRequest(format!(
                "Not found or no {permission} access"
            )));
        }
        Ok(())
    }

    async fn workflow_row(&self, id: &str) -> Result<WorkflowRow, WorkflowFail> {
        let row = sqlx::query_as::<_, WorkflowRow>(
            "SELECT id, trigger, name, description, createdAt AS created_at,
                    updatedAt AS updated_at, enabled, logging
             FROM workflow WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        row.ok_or_else(|| WorkflowFail::BadRequest("Workflow not found".into()))
    }

    async fn workflow_document(
        &self,
        row: &WorkflowRow,
        share: bool,
    ) -> Result<Value, WorkflowFail> {
        let steps = sqlx::query_as::<_, StepRow>(
            "SELECT plugin.name AS plugin_name, plugin_method.name AS method_name,
                    workflow_step.config AS config, workflow_step.enabled AS enabled
             FROM workflow_step
             JOIN plugin_method ON plugin_method.id = workflow_step.pluginMethodId
             JOIN plugin ON plugin.id = plugin_method.pluginId
             WHERE workflow_step.workflowId = ?
             ORDER BY workflow_step.\"order\"",
        )
        .bind(&row.id)
        .fetch_all(self.pool())
        .await?;
        let steps = steps
            .iter()
            .map(|step| step_json(step, share))
            .collect::<Vec<_>>();
        if share {
            return Ok(json!({
                "trigger": row.trigger,
                "name": row.name,
                "description": row.description,
                "steps": steps,
            }));
        }
        Ok(json!({
            "id": row.id,
            "trigger": row.trigger,
            "name": row.name,
            "description": row.description,
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "enabled": row.enabled != 0,
            "logging": row.logging != 0,
            "steps": steps,
        }))
    }

    async fn resolve_steps(
        &self,
        steps: Option<&Value>,
        trigger: &str,
    ) -> Result<Vec<ResolvedStep>, WorkflowFail> {
        let Some(steps) = steps else {
            return Ok(Vec::new());
        };
        let steps = steps
            .as_array()
            .ok_or_else(|| WorkflowFail::BadRequest("Validation failed".into()))?;
        let mut resolved = Vec::new();
        for step in steps {
            let step = step
                .as_object()
                .ok_or_else(|| WorkflowFail::BadRequest("Validation failed".into()))?;
            let method = step
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| WorkflowFail::BadRequest("Validation failed".into()))?;
            let Some((plugin_name, method_name)) = parse_method(method) else {
                return Err(WorkflowFail::BadRequest(format!("Unknown method {method}")));
            };
            let found = sqlx::query_as::<_, (String, String)>(
                "SELECT plugin_method.id, plugin_method.types
                 FROM plugin_method
                 JOIN plugin ON plugin.id = plugin_method.pluginId
                 WHERE plugin.name = ? AND plugin_method.name = ?",
            )
            .bind(&plugin_name)
            .bind(&method_name)
            .fetch_optional(self.pool())
            .await?;
            let Some((method_id, types)) = found else {
                return Err(WorkflowFail::BadRequest(format!("Unknown method {method}")));
            };
            let types: Value = serde_json::from_str(&types).unwrap_or_else(|_| json!([]));
            if !plugins::method_fits(&types, trigger) {
                return Err(WorkflowFail::BadRequest(format!(
                    "Method \"{method}\" is incompatible with workflow trigger: \"{trigger}\""
                )));
            }
            let config = match step.get("config") {
                None | Some(Value::Null) => None,
                Some(value) if value.is_object() => Some(value.to_string()),
                Some(_) => return Err(WorkflowFail::BadRequest("Validation failed".into())),
            };
            let enabled = match step.get("enabled") {
                None => true,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Err(WorkflowFail::BadRequest("Validation failed".into())),
            };
            resolved.push(ResolvedStep {
                method_id,
                config,
                enabled,
            });
        }
        Ok(resolved)
    }

    async fn replace_steps(
        &self,
        workflow_id: &str,
        steps: &[ResolvedStep],
    ) -> Result<(), WorkflowFail> {
        sqlx::query("DELETE FROM workflow_step WHERE workflowId = ?")
            .bind(workflow_id)
            .execute(self.pool())
            .await?;
        for (index, step) in steps.iter().enumerate() {
            sqlx::query(
                "INSERT INTO workflow_step (
                    id, enabled, workflowId, pluginMethodId, config, \"order\"
                 ) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(new_id())
            .bind(i64::from(step.enabled))
            .bind(workflow_id)
            .bind(&step.method_id)
            .bind(&step.config)
            .bind(index as i64)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }
}

struct WorkflowAsset {
    owner_id: String,
    value: Value,
}

enum StepOutcome {
    Continue,
    Halt,
    Error(String),
}

#[derive(sqlx::FromRow)]
struct RunStep {
    id: String,
    config: Option<String>,
    #[allow(dead_code)]
    step_order: i64,
    method_name: String,
    types: String,
    host_functions: i64,
    allowed_hosts: String,
    #[allow(dead_code)]
    plugin_id: String,
    plugin_enabled: i64,
    wasm_bytes: Vec<u8>,
}

#[derive(sqlx::FromRow)]
struct LogRow {
    id: String,
    created_at: String,
    result: String,
    trigger_data_id: Option<String>,
    plugin_id: Option<String>,
    method_name: Option<String>,
    step_order: Option<i64>,
}

fn log_json(row: &LogRow) -> Value {
    let mut entry = json!({
        "id": row.id,
        "at": row.created_at,
        "result": row.result,
    });
    if let Some(trigger) = &row.trigger_data_id {
        entry["triggerDataId"] = json!(trigger);
    }
    if let (Some(plugin_id), Some(method), Some(index)) =
        (&row.plugin_id, &row.method_name, row.step_order)
    {
        entry["lastStep"] = json!({
            "index": index,
            "method": format!("{plugin_id}#{method}"),
        });
    }
    entry
}

fn step_json(step: &StepRow, share: bool) -> Value {
    let config = step
        .config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null);
    let mut value = json!({
        "method": format!("{}#{}", step.plugin_name, step.method_name),
        "config": config,
    });
    if share {
        if step.enabled == 0 {
            value["enabled"] = json!(false);
        }
    } else {
        value["enabled"] = json!(step.enabled != 0);
    }
    value
}

fn parse_method(method: &str) -> Option<(String, String)> {
    let (left, method_name) = method.split_once('#')?;
    if left.is_empty()
        || method_name.is_empty()
        || left.contains(['#', ' '])
        || method_name.contains(['@', '#', ' '])
    {
        return None;
    }
    let plugin_name = left.split_once('@').map(|(name, _)| name).unwrap_or(left);
    if plugin_name.is_empty() || plugin_name.contains('@') {
        return None;
    }
    Some((plugin_name.to_string(), method_name.to_string()))
}

fn require_trigger(trigger: &str) -> Result<(), WorkflowFail> {
    if TRIGGERS.contains(&trigger) {
        Ok(())
    } else {
        Err(WorkflowFail::BadRequest("Validation failed".into()))
    }
}

fn require_uuid(value: &str) -> Result<(), WorkflowFail> {
    let ok = uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.get_version() == Some(uuid::Version::Random));
    if ok {
        Ok(())
    } else {
        Err(WorkflowFail::BadRequest("Validation failed".into()))
    }
}

fn optional_text(body: &Value, key: &str) -> Result<Option<String>, WorkflowFail> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(WorkflowFail::BadRequest("Validation failed".into())),
    }
}

fn optional_bool(body: &Value, key: &str) -> Result<Option<bool>, WorkflowFail> {
    match body.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(WorkflowFail::BadRequest("Validation failed".into())),
    }
}

fn i64_bool(value: &str) -> Result<i64, WorkflowFail> {
    match value {
        "true" => Ok(1),
        "false" => Ok(0),
        _ => Err(WorkflowFail::BadRequest("Validation failed".into())),
    }
}

fn query_map(query: &str) -> Result<BTreeMap<String, String>, WorkflowFail> {
    let mut map = BTreeMap::new();
    if query.is_empty() {
        return Ok(map);
    }
    for item in query.split('&') {
        if item.is_empty() {
            continue;
        }
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        let key = decode(key);
        if map.contains_key(&key) {
            continue;
        }
        map.insert(key, decode(value));
    }
    Ok(map)
}

fn decode(value: &str) -> String {
    let mut output = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                output.push(byte as char);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            output.push(' ');
        } else {
            output.push(bytes[index] as char);
        }
        index += 1;
    }
    output
}

pub(crate) fn workflow_query(query: &str) -> Result<BTreeMap<String, String>, WorkflowFail> {
    query_map(query)
}
