//! WASM plugins. Startup imports `plugins/immich-plugin-core` and, when the environment allows
//! it, each directory in `IMMICH_PLUGINS_INSTALL_FOLDER`. A bad manifest is skipped. Extism runs
//! the stored bytes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use extism::{CurrentPlugin, Error, Function, UserData, Val, ValType};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::StartupError;
use crate::db::{new_id, now_iso, App, DbError};

const TRIGGERS: [&str; 3] = ["AssetCreate", "AssetMetadataExtraction", "AssetTagged"];

const HOST_REFUSAL: &str = "Calling host functions is not allowed without setting methods[].hostFunctions=true in the plugin manifest";

struct ManifestMethod {
    name: String,
    title: String,
    description: String,
    types: String,
    host_functions: bool,
    allowed_hosts: String,
    schema: Option<String>,
    ui_hints: String,
}

struct ParsedManifest {
    name: String,
    version: String,
    title: String,
    description: String,
    author: String,
    wasm_path: String,
    templates: String,
    methods: Vec<ManifestMethod>,
}

pub(crate) async fn import_configured_plugins(app: &App) -> Result<(), StartupError> {
    let force = env_is_development()?;
    let _ = install_folder()?;
    if let Ok(cwd) = std::env::current_dir() {
        let core = cwd.join("plugins").join("immich-plugin-core");
        if core.is_dir() {
            if let Err(error) = import_folder(app, &core, force).await {
                eprintln!("plugin import: {error}");
            }
        }
    }
    if env_bool("IMMICH_ALLOW_EXTERNAL_PLUGINS")? {
        if let Some(folder) = install_folder()? {
            import_children(app, &folder).await;
        }
    }
    Ok(())
}

impl App {
    /// Import one plugin directory. `force` re-reads the manifest even when its hash is unchanged.
    pub async fn import_plugin_folder(&self, folder: &Path, force: bool) {
        if let Err(error) = import_folder(self, folder, force).await {
            eprintln!("plugin import: {error}");
        }
    }

    pub(crate) async fn plugin_methods(&self, trigger: Option<&str>) -> Result<Value, DbError> {
        let rows = sqlx::query_as::<_, MethodRow>(
            "SELECT plugin.name AS plugin_name, plugin_method.name AS name,
                    plugin_method.title AS title, plugin_method.description AS description,
                    plugin_method.types AS types, plugin_method.hostFunctions AS host_functions,
                    plugin_method.uiHints AS ui_hints, plugin_method.schema AS schema
             FROM plugin_method
             JOIN plugin ON plugin.id = plugin_method.pluginId
             ORDER BY plugin_method.name",
        )
        .fetch_all(self.pool())
        .await?;
        let mut methods = Vec::new();
        for row in rows {
            let types = parse_json_array(&row.types);
            if let Some(trigger) = trigger {
                if !method_fits(&types, trigger) {
                    continue;
                }
            }
            methods.push(method_json(&row));
        }
        Ok(Value::Array(methods))
    }

    pub(crate) async fn plugin_templates(&self) -> Result<Value, DbError> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT name, templates FROM plugin ORDER BY name",
        )
        .fetch_all(self.pool())
        .await?;
        let mut templates = Vec::new();
        for (plugin_name, raw) in rows {
            let Ok(list) = serde_json::from_str::<Vec<Value>>(&raw) else {
                continue;
            };
            for template in list {
                templates.push(template_json(&plugin_name, &template));
            }
        }
        Ok(Value::Array(templates))
    }

    pub(crate) async fn list_plugins(&self) -> Result<Value, DbError> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM plugin ORDER BY name")
            .fetch_all(self.pool())
            .await?;
        let mut plugins = Vec::new();
        for id in ids {
            if let Some(plugin) = self.plugin_document(&id).await? {
                plugins.push(plugin);
            }
        }
        Ok(Value::Array(plugins))
    }

    pub(crate) async fn plugin_document(&self, id: &str) -> Result<Option<Value>, DbError> {
        let Some(row) = sqlx::query_as::<_, PluginRow>(
            "SELECT id, name, title, description, author, version,
                    createdAt AS created_at, updatedAt AS updated_at
             FROM plugin WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?
        else {
            return Ok(None);
        };
        let methods = sqlx::query_as::<_, MethodRow>(
            "SELECT plugin.name AS plugin_name, plugin_method.name AS name,
                    plugin_method.title AS title, plugin_method.description AS description,
                    plugin_method.types AS types, plugin_method.hostFunctions AS host_functions,
                    plugin_method.uiHints AS ui_hints, plugin_method.schema AS schema
             FROM plugin_method
             JOIN plugin ON plugin.id = plugin_method.pluginId
             WHERE plugin_method.pluginId = ?
             ORDER BY plugin_method.name",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        Ok(Some(json!({
            "id": row.id,
            "name": row.name,
            "title": row.title,
            "description": row.description,
            "author": row.author,
            "version": row.version,
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "methods": methods.iter().map(method_json).collect::<Vec<_>>(),
        })))
    }
}

#[derive(sqlx::FromRow)]
struct PluginRow {
    id: String,
    name: String,
    title: String,
    description: String,
    author: String,
    version: String,
    created_at: String,
    updated_at: String,
}

#[derive(sqlx::FromRow)]
struct MethodRow {
    plugin_name: String,
    name: String,
    title: String,
    description: String,
    types: String,
    host_functions: i64,
    ui_hints: String,
    schema: Option<String>,
}

fn method_json(row: &MethodRow) -> Value {
    let mut value = json!({
        "key": format!("{}#{}", row.plugin_name, row.name),
        "name": row.name,
        "title": row.title,
        "description": row.description,
        "types": parse_json_array(&row.types),
        "uiHints": parse_json_array(&row.ui_hints),
        "hostFunctions": row.host_functions != 0,
    });
    if let Some(schema) = row
        .schema
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    {
        if !schema.is_null() {
            value["schema"] = schema;
        }
    }
    value
}

fn template_json(plugin_name: &str, template: &Value) -> Value {
    let name = template.get("name").and_then(Value::as_str).unwrap_or("");
    let steps = template
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| steps.iter().map(template_step).collect::<Vec<_>>())
        .unwrap_or_default();
    json!({
        "key": format!("{plugin_name}#{name}"),
        "title": template.get("title").and_then(Value::as_str).unwrap_or(""),
        "description": template.get("description").and_then(Value::as_str).unwrap_or(""),
        "trigger": template.get("trigger").and_then(Value::as_str).unwrap_or(""),
        "steps": steps,
        "uiHints": template.get("uiHints").cloned().unwrap_or_else(|| json!([])),
    })
}

fn template_step(step: &Value) -> Value {
    let mut value = json!({
        "method": step.get("method").and_then(Value::as_str).unwrap_or(""),
        "config": step.get("config").filter(|value| !value.is_null()).cloned().unwrap_or(Value::Null),
    });
    if let Some(enabled) = step.get("enabled").and_then(Value::as_bool) {
        value["enabled"] = json!(enabled);
    }
    value
}

pub(crate) fn method_fits(types: &Value, trigger: &str) -> bool {
    let Some(types) = types.as_array() else {
        return false;
    };
    if !TRIGGERS.contains(&trigger) {
        return false;
    }
    types.iter().any(|kind| kind.as_str() == Some("AssetV1"))
}

fn parse_json_array(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!([]))
}

async fn import_children(app: &App, folder: &Path) {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("plugin import: {folder:?}: {error}");
            return;
        }
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if let Err(error) = import_folder(app, &entry.path(), false).await {
                eprintln!("plugin import: {error}");
            }
        }
    }
}

async fn import_folder(app: &App, folder: &Path, force: bool) -> Result<(), String> {
    let manifest_path = folder.join("manifest.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("plugin import: {}: {error}", manifest_path.display());
            return Ok(());
        }
    };
    let hash = sha256(&bytes);
    if !force {
        let existing: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT sha256hash FROM plugin WHERE sha256hash = ?")
                .bind(&hash)
                .fetch_optional(app.pool())
                .await
                .map_err(|error| error.to_string())?;
        if existing.is_some() {
            return Ok(());
        }
    }
    let contents = String::from_utf8_lossy(&bytes);
    let value: Value = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "plugin import: invalid manifest at {}: {error}",
                manifest_path.display()
            );
            return Ok(());
        }
    };
    let manifest = match parse_manifest(&value) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "plugin import: invalid manifest at {}: {error}",
                manifest_path.display()
            );
            return Ok(());
        }
    };
    let wasm_path = folder.join(&manifest.wasm_path);
    if !wasm_stays_inside(folder, &wasm_path) {
        eprintln!(
            "plugin import: wasm path escapes {}: {}",
            folder.display(),
            manifest.wasm_path
        );
        return Ok(());
    }
    let wasm_bytes = match std::fs::read(&wasm_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("plugin import: {}: {error}", wasm_path.display());
            return Ok(());
        }
    };
    store_plugin(app, &manifest, &wasm_bytes, &hash)
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn wasm_stays_inside(folder: &Path, wasm_path: &Path) -> bool {
    let Ok(folder) = folder.canonicalize() else {
        return false;
    };
    let Ok(wasm_path) = wasm_path.canonicalize() else {
        return false;
    };
    wasm_path.starts_with(folder)
}

async fn store_plugin(
    app: &App,
    manifest: &ParsedManifest,
    wasm: &[u8],
    hash: &[u8],
) -> Result<(), DbError> {
    let now = now_iso();
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM plugin WHERE name = ?")
        .bind(&manifest.name)
        .fetch_optional(app.pool())
        .await?;
    let id = existing.clone().unwrap_or_else(new_id);
    if existing.is_some() {
        sqlx::query(
            "UPDATE plugin
             SET version = ?, title = ?, description = ?, author = ?, wasmBytes = ?,
                 templates = ?, sha256hash = ?, updatedAt = ?, enabled = 1
             WHERE id = ?",
        )
        .bind(&manifest.version)
        .bind(&manifest.title)
        .bind(&manifest.description)
        .bind(&manifest.author)
        .bind(wasm)
        .bind(&manifest.templates)
        .bind(hash)
        .bind(&now)
        .bind(&id)
        .execute(app.pool())
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO plugin (
                id, enabled, name, version, title, description, author, wasmBytes,
                templates, sha256hash, createdAt, updatedAt
             ) VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&manifest.name)
        .bind(&manifest.version)
        .bind(&manifest.title)
        .bind(&manifest.description)
        .bind(&manifest.author)
        .bind(wasm)
        .bind(&manifest.templates)
        .bind(hash)
        .bind(&now)
        .bind(&now)
        .execute(app.pool())
        .await?;
    }
    if manifest.methods.is_empty() {
        sqlx::query("DELETE FROM plugin_method WHERE pluginId = ?")
            .bind(&id)
            .execute(app.pool())
            .await?;
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", manifest.methods.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql =
        format!("DELETE FROM plugin_method WHERE pluginId = ? AND name NOT IN ({placeholders})");
    let mut query = sqlx::query(&sql).bind(&id);
    for method in &manifest.methods {
        query = query.bind(&method.name);
    }
    query.execute(app.pool()).await?;
    for method in &manifest.methods {
        let method_id = new_id();
        sqlx::query(
            "INSERT INTO plugin_method (
                id, pluginId, name, title, description, types, hostFunctions,
                allowedHosts, schema, uiHints
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(pluginId, name) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                types = excluded.types,
                hostFunctions = excluded.hostFunctions,
                allowedHosts = excluded.allowedHosts,
                schema = excluded.schema,
                uiHints = excluded.uiHints",
        )
        .bind(&method_id)
        .bind(&id)
        .bind(&method.name)
        .bind(&method.title)
        .bind(&method.description)
        .bind(&method.types)
        .bind(i64::from(method.host_functions))
        .bind(&method.allowed_hosts)
        .bind(&method.schema)
        .bind(&method.ui_hints)
        .execute(app.pool())
        .await?;
    }
    Ok(())
}

fn parse_manifest(value: &Value) -> Result<ParsedManifest, String> {
    let object = value.as_object().ok_or("manifest must be an object")?;
    let name = required_string(object, "name")?;
    if !valid_plugin_name(&name) {
        return Err("plugin name must be lowercase letters, numbers, and hyphens".into());
    }
    let version = required_string(object, "version")?;
    if !is_semver(&version) {
        return Err("plugin version must be semver".into());
    }
    let title = required_string(object, "title")?;
    let description = required_string(object, "description")?;
    let author = required_string(object, "author")?;
    let wasm_path = required_string(object, "wasmPath")?;
    if wasm_path.contains('\0') {
        return Err("wasm path is empty".into());
    }
    let mut methods = Vec::new();
    if let Some(list) = object.get("methods") {
        let list = list.as_array().ok_or("methods must be an array")?;
        for method in list {
            methods.push(parse_method(method)?);
        }
    }
    let mut templates = Vec::new();
    let mut names = Vec::new();
    if let Some(list) = object.get("templates") {
        let list = list.as_array().ok_or("templates must be an array")?;
        for template in list {
            let template = template.as_object().ok_or("template must be an object")?;
            let template_name = required_string(template, "name")?;
            if names.iter().any(|name: &String| name == &template_name) {
                return Err("template names must be unique".into());
            }
            names.push(template_name);
            let _ = required_string(template, "title")?;
            let _ = required_string(template, "description")?;
            let trigger = required_string(template, "trigger")?;
            if !TRIGGERS.contains(&trigger.as_str()) {
                return Err(format!("unknown trigger {trigger}"));
            }
            let steps = template
                .get("steps")
                .and_then(Value::as_array)
                .ok_or("template steps must be an array")?;
            for step in steps {
                let step = step.as_object().ok_or("template step must be an object")?;
                let _ = required_string(step, "method")?;
            }
            templates.push(Value::Object(template.clone()));
        }
    }
    Ok(ParsedManifest {
        name,
        version,
        title,
        description,
        author,
        wasm_path,
        templates: Value::Array(templates).to_string(),
        methods,
    })
}

fn parse_method(value: &Value) -> Result<ManifestMethod, String> {
    let object = value.as_object().ok_or("method must be an object")?;
    let name = required_string(object, "name")?;
    let title = required_string(object, "title")?;
    let description = required_string(object, "description")?;
    let types = object
        .get("types")
        .and_then(Value::as_array)
        .ok_or("method types must be an array")?;
    if types.is_empty() {
        return Err(format!("method {name} has no types"));
    }
    for kind in types {
        if kind.as_str() != Some("AssetV1") {
            return Err(format!("method {name} has an unknown type"));
        }
    }
    let host_functions = match object.get("hostFunctions") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(format!("method {name} hostFunctions must be a boolean")),
    };
    let allowed_hosts = match object.get("allowedHosts") {
        None => json!([]),
        Some(value)
            if value
                .as_array()
                .is_some_and(|items| items.iter().all(|item| item.is_string())) =>
        {
            value.clone()
        }
        Some(_) => return Err(format!("method {name} allowedHosts must be strings")),
    };
    let ui_hints = match object.get("uiHints") {
        None => json!([]),
        Some(value)
            if value
                .as_array()
                .is_some_and(|items| items.iter().all(|item| item.is_string())) =>
        {
            value.clone()
        }
        Some(_) => return Err(format!("method {name} uiHints must be strings")),
    };
    let schema = match object.get("schema") {
        None | Some(Value::Null) => None,
        Some(value) if value.as_object().is_some_and(|object| object.is_empty()) => None,
        Some(value) if value.is_object() => Some(value.to_string()),
        Some(_) => return Err(format!("method {name} schema must be an object")),
    };
    Ok(ManifestMethod {
        name,
        title,
        description,
        types: Value::Array(types.clone()).to_string(),
        host_functions,
        allowed_hosts: allowed_hosts.to_string(),
        schema,
        ui_hints: ui_hints.to_string(),
    })
}

fn required_string(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    match object.get(key).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(format!("{key} is required")),
    }
}

fn valid_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn is_semver(value: &str) -> bool {
    let core = value.split_once('+').map(|(core, _)| core).unwrap_or(value);
    let (numbers, pre) = core.split_once('-').unwrap_or((core, ""));
    let mut parts = numbers.split('.');
    let Some(major) = parts.next() else {
        return false;
    };
    let Some(minor) = parts.next() else {
        return false;
    };
    let Some(patch) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || !numeric_id(major) || !numeric_id(minor) || !numeric_id(patch) {
        return false;
    }
    pre.is_empty() || pre.split('.').all(pre_id)
}

fn numeric_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn pre_id(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return numeric_id(value);
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

fn env_is_development() -> Result<bool, StartupError> {
    match std::env::var("IMMICH_ENV") {
        Err(_) => Ok(false),
        Ok(value) if value.trim().is_empty() => Ok(false),
        Ok(value) if matches!(value.as_str(), "development" | "testing" | "production") => {
            Ok(value == "development")
        }
        Ok(_) => Err(StartupError::Invalid { name: "IMMICH_ENV" }),
    }
}

fn env_bool(name: &'static str) -> Result<bool, StartupError> {
    match std::env::var(name) {
        Err(_) => Ok(false),
        Ok(value) if value.trim().is_empty() => Ok(false),
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(StartupError::Invalid { name }),
        },
    }
}

fn install_folder() -> Result<Option<PathBuf>, StartupError> {
    match std::env::var("IMMICH_PLUGINS_INSTALL_FOLDER") {
        Err(_) => Ok(None),
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(StartupError::Invalid {
                    name: "IMMICH_PLUGINS_INSTALL_FOLDER",
                });
            }
            Ok(Some(path))
        }
    }
}

pub(crate) struct PluginCall {
    pub wasm: Vec<u8>,
    pub method: String,
    pub input: String,
    pub allow_host: bool,
    pub user_id: String,
    pub allowed_hosts: Vec<String>,
    pub depth: u32,
}

struct HostGate {
    allow: bool,
    handle: tokio::runtime::Handle,
    app: App,
    user_id: String,
    allowed_hosts: Vec<String>,
    depth: u32,
}

pub(crate) async fn call_plugin(app: &App, call: PluginCall) -> Result<Option<String>, String> {
    let handle = tokio::runtime::Handle::current();
    let app = app.clone();
    tokio::task::spawn_blocking(move || call_plugin_sync(&app, &handle, call))
        .await
        .map_err(|error| error.to_string())?
}

fn call_plugin_sync(
    app: &App,
    handle: &tokio::runtime::Handle,
    call: PluginCall,
) -> Result<Option<String>, String> {
    let gate = UserData::new(HostGate {
        allow: call.allow_host,
        handle: handle.clone(),
        app: app.clone(),
        user_id: call.user_id,
        allowed_hosts: call.allowed_hosts,
        depth: call.depth,
    });
    let functions = [
        "searchAlbums",
        "createAlbum",
        "addAssetsToAlbum",
        "addAssetsToAlbums",
        "httpRequest",
        "bulkTagAssets",
    ]
    .map(|name| host_function(name, gate.clone()));
    let mut plugin = extism::Plugin::new(call.wasm.as_slice(), functions, true)
        .map_err(|error| error.to_string())?;
    let output = plugin
        .call::<&str, String>(&call.method, call.input.as_str())
        .map_err(|error| error.to_string())?;
    if output.is_empty() {
        Ok(None)
    } else {
        Ok(Some(output))
    }
}

fn host_function(name: &'static str, gate: UserData<HostGate>) -> Function {
    Function::new(
        name,
        [ValType::I64],
        [ValType::I64],
        gate,
        move |plugin, inputs, outputs, user_data| {
            dispatch_host(name, plugin, inputs, outputs, &user_data)
        },
    )
    .with_namespace("extism:host/user")
}

fn dispatch_host(
    name: &str,
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    user_data: &UserData<HostGate>,
) -> Result<(), Error> {
    let gate = user_data.get()?;
    let (allow, handle, app, user_id, allowed_hosts, depth) = {
        let gate = gate.lock().map_err(|_| Error::msg("host lock"))?;
        (
            gate.allow,
            gate.handle.clone(),
            gate.app.clone(),
            gate.user_id.clone(),
            gate.allowed_hosts.clone(),
            gate.depth,
        )
    };
    if !allow {
        return Err(Error::msg(HOST_REFUSAL));
    }
    let text = match inputs.first() {
        Some(input) => plugin.memory_get_val::<String>(input).unwrap_or_default(),
        None => String::new(),
    };
    let response = if text.is_empty() {
        fail_body(400, "Called host function without input")
    } else {
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let args = parsed.get("args").cloned().unwrap_or_else(|| json!([]));
        match handle.block_on(run_host(&app, &user_id, depth, &allowed_hosts, name, &args)) {
            Ok(value) => json!({"success": true, "response": value}).to_string(),
            Err(error) => fail_body(400, &error),
        }
    };
    plugin.memory_set_val(&mut outputs[0], response)?;
    Ok(())
}

fn fail_body(status: u16, message: &str) -> String {
    json!({"success": false, "status": status, "message": message}).to_string()
}

async fn run_host(
    app: &App,
    user_id: &str,
    depth: u32,
    allowed_hosts: &[String],
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let args = args.as_array().cloned().unwrap_or_default();
    match name {
        "searchAlbums" => search_albums(app, user_id, args.first()).await,
        "createAlbum" => create_album(app, user_id, args.first()).await,
        "addAssetsToAlbum" => add_assets_to_album(app, user_id, &args).await,
        "addAssetsToAlbums" => add_assets_to_albums(app, user_id, args.first()).await,
        "httpRequest" => http_request(allowed_hosts, &args).await,
        "bulkTagAssets" => {
            let body = args.first().cloned().unwrap_or_else(|| json!({}));
            app.bulk_tag_assets_at(user_id, &body, depth.saturating_add(1))
                .await
                .map_err(|error| format!("{error:?}"))
        }
        _ => Err(format!("unknown host function {name}")),
    }
}

async fn search_albums(app: &App, user_id: &str, dto: Option<&Value>) -> Result<Value, String> {
    let name = dto
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str);
    let rows = if let Some(name) = name {
        sqlx::query_as::<_, (String, String)>(
            "SELECT id, albumName FROM album
             WHERE ownerId = ? AND deletedAt IS NULL AND albumName = ?",
        )
        .bind(user_id)
        .bind(name)
        .fetch_all(app.pool())
        .await
    } else {
        sqlx::query_as::<_, (String, String)>(
            "SELECT id, albumName FROM album WHERE ownerId = ? AND deletedAt IS NULL",
        )
        .bind(user_id)
        .fetch_all(app.pool())
        .await
    }
    .map_err(|error| error.to_string())?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(id, album_name)| json!({"id": id, "albumName": album_name}))
            .collect(),
    ))
}

async fn create_album(app: &App, user_id: &str, dto: Option<&Value>) -> Result<Value, String> {
    let name = dto
        .and_then(|value| value.get("albumName"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let asset_ids = string_array(dto.and_then(|value| value.get("assetIds")));
    app.create_album(user_id, name, None, &asset_ids)
        .await
        .map_err(|error| format!("{error:?}"))
}

async fn add_assets_to_album(app: &App, user_id: &str, args: &[Value]) -> Result<Value, String> {
    let album_id = args.first().and_then(Value::as_str).unwrap_or("");
    let asset_ids = string_array(args.get(1).and_then(|value| value.get("ids")));
    app.add_album_assets(user_id, album_id, &asset_ids)
        .await
        .map_err(|error| format!("{error:?}"))
}

async fn add_assets_to_albums(
    app: &App,
    user_id: &str,
    dto: Option<&Value>,
) -> Result<Value, String> {
    let album_ids = string_array(dto.and_then(|value| value.get("albumIds")));
    let asset_ids = string_array(dto.and_then(|value| value.get("assetIds")));
    let mut results = Vec::new();
    for album_id in &album_ids {
        let added = app
            .add_album_assets(user_id, album_id, &asset_ids)
            .await
            .map_err(|error| format!("{error:?}"))?;
        results.push(added);
    }
    Ok(Value::Array(results))
}

async fn http_request(allowed_hosts: &[String], args: &[Value]) -> Result<Value, String> {
    let url = args.first().and_then(Value::as_str).unwrap_or("");
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid url".to_string())?;
    let hostname = parsed.host_str().unwrap_or("");
    if !allowed_hosts
        .iter()
        .any(|pattern| host_matches(hostname, pattern))
    {
        return Err(
            "Hostname did not match any listed in methods[].allowedHosts in the plugin manifest"
                .to_string(),
        );
    }
    let options = args.get(1);
    let method = options
        .and_then(|value| value.get("method"))
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let method = method.parse().map_err(|_| "invalid method".to_string())?;
    let mut request = client.request(method, parsed);
    if let Some(headers) = options
        .and_then(|value| value.get("headers"))
        .and_then(Value::as_object)
    {
        for (name, value) in headers {
            if let Some(value) = value.as_str() {
                request = request.header(name, value);
            }
        }
    }
    if let Some(body) = options
        .and_then(|value| value.get("body"))
        .and_then(Value::as_str)
    {
        request = request.body(body.to_string());
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    Ok(json!({
        "ok": status.is_success(),
        "status": status.as_u16(),
        "body": body,
    }))
}

fn host_matches(hostname: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let regex = pattern.replace('.', r"\.").replace('*', ".*");
    regex_substring(&regex, hostname)
}

/// Unanchored match for a pattern of literals and `.*` only.
fn regex_substring(pattern: &str, hostname: &str) -> bool {
    let parts: Vec<&str> = pattern.split(".*").collect();
    if parts.len() == 1 {
        return hostname.contains(&parts[0].replace(r"\.", "."));
    }
    let mut rest = hostname;
    for (index, part) in parts.iter().enumerate() {
        let literal = part.replace(r"\.", ".");
        if literal.is_empty() {
            continue;
        }
        if index == 0 {
            if !rest.starts_with(&literal) {
                return false;
            }
            rest = &rest[literal.len()..];
            continue;
        }
        if index + 1 == parts.len() && !pattern.ends_with(".*") {
            return rest.ends_with(&literal);
        }
        let Some(position) = rest.find(&literal) else {
            return false;
        };
        rest = &rest[position + literal.len()..];
    }
    true
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
