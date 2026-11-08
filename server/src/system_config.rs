//! The v3.3.6 admin config document. Object-storage credentials are not keys of this document.
//! `email` and `realtimeTranscoding` on the feature list stay false even when this document says otherwise.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Map, Value};
use sqlx::SqlitePool;

use crate::config::StartupError;
use crate::db::{App, DbError};

const DEFAULTS_JSON: &str = include_str!("system_config_defaults.json");
const METADATA_KEY: &str = "system-config";

pub(crate) enum ConfigError {
    Db(DbError),
    Message(String),
}

impl ConfigError {
    fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

impl From<DbError> for ConfigError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

const CLIP_MODELS: &[&str] = &[
    "RN101__openai",
    "RN101__yfcc15m",
    "ViT-B-16__laion400m_e31",
    "ViT-B-16__laion400m_e32",
    "ViT-B-16__openai",
    "ViT-B-32__laion2b-s34b-b79k",
    "ViT-B-32__laion2b_e16",
    "ViT-B-32__laion400m_e31",
    "ViT-B-32__laion400m_e32",
    "ViT-B-32__openai",
    "XLM-Roberta-Base-ViT-B-32__laion5b_s13b_b90k",
    "XLM-Roberta-Large-Vit-B-32",
    "RN50x4__openai",
    "ViT-B-16-plus-240__laion400m_e31",
    "ViT-B-16-plus-240__laion400m_e32",
    "XLM-Roberta-Large-Vit-B-16Plus",
    "LABSE-Vit-L-14",
    "RN50x16__openai",
    "ViT-B-16-SigLIP-256__webli",
    "ViT-B-16-SigLIP-384__webli",
    "ViT-B-16-SigLIP-512__webli",
    "ViT-B-16-SigLIP-i18n-256__webli",
    "ViT-B-16-SigLIP__webli",
    "ViT-L-14-336__openai",
    "ViT-L-14-quickgelu__dfn2b",
    "ViT-L-14__laion2b-s32b-b82k",
    "ViT-L-14__laion400m_e31",
    "ViT-L-14__laion400m_e32",
    "ViT-L-14__openai",
    "XLM-Roberta-Large-Vit-L-14",
    "nllb-clip-base-siglip__mrl",
    "nllb-clip-base-siglip__v1",
    "RN50__cc12m",
    "RN50__openai",
    "RN50__yfcc15m",
    "RN50x64__openai",
    "ViT-H-14-378-quickgelu__dfn5b",
    "ViT-H-14-quickgelu__dfn5b",
    "ViT-H-14__laion2b-s32b-b79k",
    "ViT-L-16-SigLIP-256__webli",
    "ViT-L-16-SigLIP-384__webli",
    "ViT-g-14__laion2b-s12b-b42k",
    "XLM-Roberta-Large-ViT-H-14__frozen_laion5b_s13b_b90k",
    "ViT-SO400M-14-SigLIP-384__webli",
    "nllb-clip-large-siglip__mrl",
    "nllb-clip-large-siglip__v1",
    "ViT-B-16-SigLIP2__webli",
    "ViT-B-32-SigLIP2-256__webli",
    "ViT-L-16-SigLIP2-256__webli",
    "ViT-L-16-SigLIP2-384__webli",
    "ViT-L-16-SigLIP2-512__webli",
    "ViT-SO400M-14-SigLIP2__webli",
    "ViT-SO400M-14-SigLIP2-378__webli",
    "ViT-SO400M-16-SigLIP2-256__webli",
    "ViT-SO400M-16-SigLIP2-384__webli",
    "ViT-SO400M-16-SigLIP2-512__webli",
    "ViT-gopt-16-SigLIP2-256__webli",
    "ViT-gopt-16-SigLIP2-384__webli",
];

/// Embedding length for each name in `CLIP_MODELS`, from v3.3.6 `CLIP_MODEL_INFO`.
const CLIP_DIMS: &[usize] = &[
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 640, 640, 640, 640, 768, 768, 768,
    768, 768, 768, 768, 768, 768, 768, 768, 768, 768, 768, 768, 768, 1024, 1024, 1024, 1024, 1024,
    1024, 1024, 1024, 1024, 1024, 1024, 1152, 1152, 1152, 768, 768, 1024, 1024, 1024, 1152, 1152,
    1152, 1152, 1152, 1536, 1536,
];

const _: () = assert!(CLIP_MODELS.len() == CLIP_DIMS.len());

pub(crate) fn clip_dimensions(name: &str) -> Option<usize> {
    let token = name.rsplit('/').next().unwrap_or("");
    if token.is_empty() {
        return None;
    }
    let cleaned = token.replace(':', "_");
    let index = CLIP_MODELS.iter().position(|model| *model == cleaned)?;
    CLIP_DIMS.get(index).copied()
}

const DATE_TOKENS: &[&str] = &[
    "s", "ss", "SSS", "m", "mm", "d", "dd", "W", "WW", "h", "hh", "H", "HH", "y", "yy", "M", "MM",
    "MMM", "MMMM",
];

pub(crate) fn storage_template_options() -> Value {
    json!({
        "secondOptions": ["s", "ss", "SSS"],
        "minuteOptions": ["m", "mm"],
        "dayOptions": ["d", "dd"],
        "weekOptions": ["W", "WW"],
        "hourOptions": ["h", "hh", "H", "HH"],
        "yearOptions": ["y", "yy"],
        "monthOptions": ["M", "MM", "MMM", "MMMM"],
        "presetOptions": [
            "{{y}}/{{y}}-{{MM}}-{{dd}}/{{filename}}",
            "{{y}}/{{MM}}-{{dd}}/{{filename}}",
            "{{y}}/{{MMMM}}-{{dd}}/{{filename}}",
            "{{y}}/{{MM}}/{{filename}}",
            "{{y}}/{{#if album}}{{album}}{{else}}Other/{{MM}}{{/if}}/{{filename}}",
            "{{#if album}}{{album-startDate-y}}/{{album}}{{else}}{{y}}/Other/{{MM}}{{/if}}/{{filename}}",
            "{{y}}/{{MMM}}/{{filename}}",
            "{{y}}/{{MMMM}}/{{filename}}",
            "{{y}}/{{MM}}/{{dd}}/{{filename}}",
            "{{y}}/{{MMMM}}/{{dd}}/{{filename}}",
            "{{y}}/{{y}}-{{MM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}",
            "{{y}}-{{MM}}-{{dd}}/{{filename}}",
            "{{y}}-{{MMM}}-{{dd}}/{{filename}}",
            "{{y}}-{{MMMM}}-{{dd}}/{{filename}}",
            "{{y}}/{{y}}-{{MM}}/{{filename}}",
            "{{y}}/{{y}}-{{WW}}/{{filename}}",
            "{{y}}/{{y}}-{{MM}}-{{dd}}/{{assetId}}",
            "{{y}}/{{y}}-{{MM}}/{{assetId}}",
            "{{y}}/{{y}}-{{WW}}/{{assetId}}",
            "{{album}}/{{filename}}",
            "{{make}}/{{model}}/{{lensModel}}/{{filename}}"
        ]
    })
}

impl App {
    pub(crate) fn config_defaults(&self) -> Value {
        let mut defaults = base_defaults();
        if let Some(machine_learning) = defaults.get_mut("machineLearning") {
            machine_learning["enabled"] = json!(self.machine_learning_enabled());
            machine_learning["urls"] = json!([self.machine_learning_url()]);
        }
        defaults
    }

    pub(crate) async fn admin_config(&self) -> Result<Value, DbError> {
        let defaults = self.config_defaults();
        let partial = self.stored_config().await?;
        Ok(finish_config(merge(&defaults, &partial)))
    }

    pub(crate) async fn replace_admin_config(&self, body: &Value) -> Result<Value, ConfigError> {
        if self.config_file_locked() {
            return Err(ConfigError::message(
                "Cannot update configuration while IMMICH_CONFIG_FILE is in use",
            ));
        }
        let defaults = self.config_defaults();
        let current = self.admin_config().await?;
        let incoming = document_or_error(&defaults, body)?;
        validate_config(self, &current, &incoming).await?;
        let partial = diff_from_defaults(&defaults, &incoming);
        self.write_config(&partial).await?;
        let saved = finish_config(merge(&defaults, &partial));
        clear_clip_on_model_change(self, &current, &saved).await;
        Ok(saved)
    }

    pub(crate) fn feature_flags(&self, config: &Value) -> Value {
        let machine_learning = &config["machineLearning"];
        let enabled = machine_learning["enabled"].as_bool().unwrap_or(false);
        let clip = enabled
            && machine_learning["clip"]["enabled"]
                .as_bool()
                .unwrap_or(false);
        json!({
            "smartSearch": clip,
            "duplicateDetection": clip && machine_learning["duplicateDetection"]["enabled"].as_bool().unwrap_or(false),
            "configFile": self.config_file_locked(),
            "facialRecognition": enabled && machine_learning["facialRecognition"]["enabled"].as_bool().unwrap_or(false),
            "map": config["map"]["enabled"].as_bool().unwrap_or(false),
            "trash": config["trash"]["enabled"].as_bool().unwrap_or(false),
            "reverseGeocoding": config["reverseGeocoding"]["enabled"].as_bool().unwrap_or(false),
            "importFaces": config["metadata"]["faces"]["import"].as_bool().unwrap_or(false),
            "oauth": config["oauth"]["enabled"].as_bool().unwrap_or(false),
            "oauthAutoLaunch": config["oauth"]["autoLaunch"].as_bool().unwrap_or(false),
            "passwordLogin": config["passwordLogin"]["enabled"].as_bool().unwrap_or(true),
            "sidecar": true,
            "search": true,
            "email": false,
            "ocr": enabled && machine_learning["ocr"]["enabled"].as_bool().unwrap_or(false),
            "realtimeTranscoding": false,
        })
    }

    pub(crate) fn server_config_body(&self, config: &Value, onboarded: bool) -> Value {
        json!({
            "loginPageMessage": config["server"]["loginPageMessage"].as_str().unwrap_or(""),
            "trashDays": config["trash"]["days"].as_i64().unwrap_or(30),
            "userDeleteDelay": config["user"]["deleteDelay"].as_i64().unwrap_or(7),
            "oauthButtonText": config["oauth"]["buttonText"].as_str().unwrap_or("Login with OAuth"),
            "oauthAccountManagementUrl": config["oauth"]["accountManagementUrl"].as_str().unwrap_or(""),
            "isInitialized": true,
            "isOnboarded": onboarded,
            "externalDomain": config["server"]["externalDomain"].as_str().unwrap_or(""),
            "publicUsers": config["server"]["publicUsers"].as_bool().unwrap_or(true),
            "mapDarkStyleUrl": config["map"]["darkStyle"].as_str().unwrap_or(""),
            "mapLightStyleUrl": config["map"]["lightStyle"].as_str().unwrap_or(""),
            "maintenanceMode": false,
            "minFaces": config["machineLearning"]["facialRecognition"]["minFaces"].as_i64().unwrap_or(3),
        })
    }

    async fn stored_config(&self) -> Result<Value, DbError> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM system_metadata WHERE key = ?")
                .bind(METADATA_KEY)
                .fetch_optional(self.pool())
                .await?;
        let Some(value) = value else {
            return Ok(json!({}));
        };
        Ok(serde_json::from_str(&value).unwrap_or_else(|_| json!({})))
    }

    async fn write_config(&self, partial: &Value) -> Result<(), DbError> {
        let text = partial.to_string();
        sqlx::query(
            "INSERT INTO system_metadata (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(METADATA_KEY)
        .bind(text)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

pub(crate) async fn install_config_file(
    pool: &SqlitePool,
    path: &Path,
) -> Result<(), StartupError> {
    let text = std::fs::read_to_string(path).map_err(StartupError::Io)?;
    let partial = parse_yaml(&text).map_err(|_| StartupError::Invalid {
        name: "IMMICH_CONFIG_FILE",
    })?;
    let text = partial.to_string();
    sqlx::query(
        "INSERT INTO system_metadata (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(METADATA_KEY)
    .bind(text)
    .execute(pool)
    .await
    .map_err(StartupError::Database)?;
    Ok(())
}

fn base_defaults() -> Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED
        .get_or_init(|| serde_json::from_str(DEFAULTS_JSON).expect("admin config defaults parse"))
        .clone()
}

fn document_or_error(defaults: &Value, body: &Value) -> Result<Value, ConfigError> {
    let Value::Object(_) = body else {
        return Err(ConfigError::message("Invalid system config"));
    };
    let mut leaves = Vec::new();
    collect_leaves(defaults, "", &mut leaves);
    for (path, default_value) in leaves {
        let Some(incoming) = get_path(body, &path) else {
            return Err(ConfigError::message("Invalid system config"));
        };
        if !same_leaf(&path, default_value, incoming) {
            return Err(ConfigError::message("Invalid system config"));
        }
    }
    if let Some(domain) = body["server"]["externalDomain"].as_str() {
        normalize_domain(domain).map_err(ConfigError::message)?;
    }
    Ok(body.clone())
}

fn same_leaf(path: &str, default_value: &Value, incoming: &Value) -> bool {
    if path == "oauth.defaultStorageQuota" {
        return incoming.is_null()
            || incoming.as_i64().is_some_and(|number| number >= 0)
            || incoming.as_u64().is_some();
    }
    match (default_value, incoming) {
        (Value::Bool(_), Value::Bool(_)) | (Value::String(_), Value::String(_)) => true,
        (Value::Number(default_number), Value::Number(incoming_number)) => {
            if default_number.is_f64() && !default_number.is_i64() {
                incoming_number.as_f64().is_some()
            } else {
                incoming_number.as_i64().is_some() || incoming_number.as_u64().is_some()
            }
        }
        (Value::Array(defaults), Value::Array(incoming)) => match defaults.first() {
            Some(Value::Number(_)) => incoming.iter().all(|item| item.is_number()),
            Some(Value::String(_)) => incoming.iter().all(|item| item.is_string()),
            Some(_) => false,
            None => true,
        },
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

async fn validate_config(app: &App, current: &Value, incoming: &Value) -> Result<(), ConfigError> {
    let template = incoming["storageTemplate"]["template"]
        .as_str()
        .unwrap_or("");
    if !valid_storage_template(template) {
        return Err(ConfigError::message("Invalid storage template"));
    }
    let model = incoming["machineLearning"]["clip"]["modelName"]
        .as_str()
        .unwrap_or("");
    if !known_clip_model(model) {
        return Err(ConfigError::message(format!(
            "Unknown CLIP model: {model}. Please check the model name for typos and confirm this is a supported model."
        )));
    }
    if app.log_level_locked() && incoming["logging"] != current["logging"] {
        return Err(ConfigError::message(
            "Logging cannot be changed while the environment variable IMMICH_LOG_LEVEL is set.",
        ));
    }
    let smtp_on = incoming["notifications"]["smtp"]["enabled"]
        .as_bool()
        .unwrap_or(false);
    if smtp_on && incoming["notifications"]["smtp"] != current["notifications"]["smtp"] {
        verify_smtp(&incoming["notifications"]["smtp"]["transport"]).await?;
    }
    Ok(())
}

async fn verify_smtp(transport: &Value) -> Result<(), ConfigError> {
    let host = transport["host"].as_str().unwrap_or("").trim();
    let port = transport["port"].as_u64().unwrap_or(0);
    if host.is_empty() || port == 0 || port > u16::MAX as u64 {
        return Err(ConfigError::message("Invalid SMTP configuration"));
    }
    let address = format!("{host}:{port}");
    match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&address),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        _ => Err(ConfigError::message("Invalid SMTP configuration")),
    }
}

async fn clear_clip_on_model_change(app: &App, previous: &Value, saved: &Value) {
    let enabled = saved["machineLearning"]["enabled"]
        .as_bool()
        .unwrap_or(false)
        && saved["machineLearning"]["clip"]["enabled"]
            .as_bool()
            .unwrap_or(false);
    let previous_name = previous["machineLearning"]["clip"]["modelName"].as_str();
    let saved_name = saved["machineLearning"]["clip"]["modelName"].as_str();
    if !enabled || previous_name == saved_name {
        return;
    }
    let table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'asset_clip'",
    )
    .fetch_optional(app.pool())
    .await
    .ok()
    .flatten();
    if table.is_none() {
        return;
    }
    let _ = sqlx::query("DELETE FROM asset_clip")
        .execute(app.pool())
        .await;
}

fn known_clip_model(name: &str) -> bool {
    let Some(token) = name.rsplit('/').next() else {
        return false;
    };
    if token.is_empty() {
        return false;
    }
    let cleaned = token.replace(':', "_");
    CLIP_MODELS.contains(&cleaned.as_str())
}

fn valid_storage_template(template: &str) -> bool {
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        if index + 1 >= bytes.len() || bytes[index + 1] != b'{' {
            return false;
        }
        let Some(end) = template[index + 2..].find("}}") else {
            return false;
        };
        let token = template[index + 2..index + 2 + end].trim();
        if !known_template_token(token) {
            return false;
        }
        index += 2 + end + 2;
    }
    true
}

fn known_template_token(token: &str) -> bool {
    let trimmed = token.trim();
    let closing = trimmed.starts_with('/');
    let token = trimmed.trim_start_matches(['#', '/']).trim();
    if token.is_empty() {
        return false;
    }
    let mut parts = token.split_whitespace();
    let name = parts.next().unwrap_or("");
    let argument = parts.next();
    if parts.next().is_some() {
        return false;
    }
    if matches!(name, "if" | "unless" | "each" | "with") {
        if closing {
            return argument.is_none();
        }
        return argument.is_some_and(known_template_name);
    }
    if name == "else" {
        return argument.is_none();
    }
    argument.is_none() && known_template_name(name)
}

fn known_template_name(name: &str) -> bool {
    matches!(
        name,
        "filename"
            | "ext"
            | "filetype"
            | "filetypefull"
            | "assetId"
            | "assetIdShort"
            | "album"
            | "make"
            | "model"
            | "lensModel"
    ) || DATE_TOKENS.contains(&name)
        || album_date_token(name)
}

fn album_date_token(name: &str) -> bool {
    let Some(token) = name
        .strip_prefix("album-startDate-")
        .or_else(|| name.strip_prefix("album-endDate-"))
    else {
        return false;
    };
    DATE_TOKENS.contains(&token)
}

fn finish_config(mut config: Value) -> Value {
    if let Some(domain) = config["server"]["externalDomain"].as_str() {
        if let Ok(normalized) = normalize_domain(domain) {
            config["server"]["externalDomain"] = json!(normalized);
        }
    }
    include_target_codec(&mut config, "targetVideoCodec", "acceptedVideoCodecs");
    include_target_codec(&mut config, "targetAudioCodec", "acceptedAudioCodecs");
    config
}

fn include_target_codec(config: &mut Value, target_key: &str, accepted_key: &str) {
    let Some(target) = config["ffmpeg"][target_key].as_str().map(str::to_string) else {
        return;
    };
    let Some(accepted) = config["ffmpeg"][accepted_key].as_array_mut() else {
        return;
    };
    if !accepted
        .iter()
        .any(|item| item.as_str() == Some(target.as_str()))
    {
        accepted.push(json!(target));
    }
}

fn normalize_domain(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let (scheme, rest) = if let Some(rest) = value.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = value.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err("External domain must be an empty string or a valid URL".to_string());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(' ') {
        return Err("External domain must be an empty string or a valid URL".to_string());
    }
    let (userinfo, host) = match authority.rsplit_once('@') {
        Some((userinfo, host)) if !host.is_empty() => (Some(userinfo), host),
        _ => (None, authority),
    };
    if host.is_empty() {
        return Err("External domain must be an empty string or a valid URL".to_string());
    }
    if let Some(userinfo) = userinfo {
        if let Some((user, password)) = userinfo.split_once(':') {
            if !user.is_empty() && !password.is_empty() {
                return Ok(format!("{scheme}://{user}:{password}@{host}"));
            }
        }
    }
    Ok(format!("{scheme}://{host}"))
}

fn diff_from_defaults(defaults: &Value, incoming: &Value) -> Value {
    let mut leaves = Vec::new();
    collect_leaves(defaults, "", &mut leaves);
    let mut partial = json!({});
    for (path, default_value) in leaves {
        let Some(incoming_value) = get_path(incoming, &path) else {
            continue;
        };
        if incoming_value.is_null() || incoming_value.as_str().is_some_and(str::is_empty) {
            continue;
        }
        if incoming_value == default_value {
            continue;
        }
        set_path(&mut partial, &path, incoming_value.clone());
    }
    partial
}

fn merge(defaults: &Value, partial: &Value) -> Value {
    let mut merged = defaults.clone();
    let mut leaves = Vec::new();
    collect_leaves(partial, "", &mut leaves);
    for (path, value) in leaves {
        set_path(&mut merged, &path, value.clone());
    }
    merged
}

fn collect_leaves<'a>(value: &'a Value, path: &str, out: &mut Vec<(String, &'a Value)>) {
    let Value::Object(map) = value else {
        return;
    };
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for key in keys {
        let child = &map[key];
        let next = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if child.is_object() {
            collect_leaves(child, &next, out);
        } else {
            out.push((next, child));
        }
    }
}

fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn set_path(node: &mut Value, path: &str, value: Value) {
    let Some((key, rest)) = path.split_once('.') else {
        if let Value::Object(map) = node {
            map.insert(path.to_string(), value);
        }
        return;
    };
    if let Value::Object(map) = node {
        let child = map.entry(key.to_string()).or_insert_with(|| json!({}));
        set_path(child, rest, value);
    }
}

pub(crate) fn parse_yaml(text: &str) -> Result<Value, String> {
    let lines = yaml_lines(text)?;
    let (value, next) = parse_block(&lines, 0)?;
    if next != lines.len() {
        return Err("yaml did not consume every line".to_string());
    }
    Ok(value)
}

struct YamlLine {
    indent: usize,
    text: String,
}

fn yaml_lines(text: &str) -> Result<Vec<YamlLine>, String> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        if raw.starts_with('\t') {
            return Err("yaml tabs are not supported".to_string());
        }
        let indent = raw
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        let text = strip_yaml_comment(raw.trim()).trim().to_string();
        if text.is_empty() {
            continue;
        }
        lines.push(YamlLine { indent, text });
    }
    Ok(lines)
}

fn strip_yaml_comment(text: &str) -> &str {
    let mut quote: Option<char> = None;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let character = bytes[index] as char;
        if quote.is_some() {
            if character == quote.unwrap() {
                quote = None;
            }
            index += 1;
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
            index += 1;
            continue;
        }
        if character == '#' && (index == 0 || bytes[index - 1] == b' ') {
            return text[..index].trim_end();
        }
        index += 1;
    }
    text
}

fn parse_block(lines: &[YamlLine], index: usize) -> Result<(Value, usize), String> {
    if index >= lines.len() {
        return Ok((json!({}), index));
    }
    if lines[index].text.starts_with("- ") || lines[index].text == "-" {
        parse_list(lines, index)
    } else {
        parse_map(lines, index)
    }
}

fn parse_map(lines: &[YamlLine], index: usize) -> Result<(Value, usize), String> {
    let indent = lines[index].indent;
    let mut map = Map::new();
    let mut cursor = index;
    while cursor < lines.len()
        && lines[cursor].indent == indent
        && !lines[cursor].text.starts_with('-')
    {
        let (key, inline) = split_yaml_key(&lines[cursor].text)?;
        cursor += 1;
        let value = if inline.is_empty() {
            if cursor < lines.len() && lines[cursor].indent > indent {
                let (child, next) = parse_block(lines, cursor)?;
                cursor = next;
                child
            } else {
                Value::Null
            }
        } else {
            parse_scalar(inline)?
        };
        map.insert(key, value);
    }
    Ok((Value::Object(map), cursor))
}

fn parse_list(lines: &[YamlLine], index: usize) -> Result<(Value, usize), String> {
    let indent = lines[index].indent;
    let mut items = Vec::new();
    let mut cursor = index;
    while cursor < lines.len()
        && lines[cursor].indent == indent
        && (lines[cursor].text.starts_with("- ") || lines[cursor].text == "-")
    {
        let rest = lines[cursor].text.trim_start_matches("- ").trim();
        let rest = if lines[cursor].text == "-" { "" } else { rest };
        cursor += 1;
        if rest.is_empty() {
            if cursor < lines.len() && lines[cursor].indent > indent {
                let (child, next) = parse_block(lines, cursor)?;
                cursor = next;
                items.push(child);
            } else {
                items.push(Value::Null);
            }
        } else {
            items.push(parse_scalar(rest)?);
        }
    }
    Ok((Value::Array(items), cursor))
}

fn split_yaml_key(text: &str) -> Result<(String, &str), String> {
    let Some((key, value)) = text.split_once(':') else {
        return Err(format!("yaml key missing: {text}"));
    };
    let key = key.trim();
    if key.is_empty() {
        return Err("yaml key is empty".to_string());
    }
    Ok((key.to_string(), value.trim()))
}

fn parse_scalar(text: &str) -> Result<Value, String> {
    if text == "null" || text == "~" || text == "Null" {
        return Ok(Value::Null);
    }
    if text == "true" || text == "True" {
        return Ok(Value::Bool(true));
    }
    if text == "false" || text == "False" {
        return Ok(Value::Bool(false));
    }
    if let Some(quoted) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return Ok(Value::String(quoted.to_string()));
    }
    if let Some(quoted) = text
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Ok(Value::String(quoted.to_string()));
    }
    if let Ok(number) = text.parse::<i64>() {
        return Ok(json!(number));
    }
    if text.contains('.') {
        if let Ok(number) = text.parse::<f64>() {
            return Ok(json!(number));
        }
    }
    Ok(Value::String(text.to_string()))
}
