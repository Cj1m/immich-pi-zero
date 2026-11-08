use std::env;
use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

/// Object storage. Unset variables keep Originals in `{data}/objects` so tests boot with no bucket.
#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub path_style: bool,
}

/// Process configuration. The first admin and the data directory come from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub web_root: PathBuf,
    pub host: IpAddr,
    pub port: u16,
    pub admin_email: Option<String>,
    pub admin_name: Option<String>,
    pub admin_password: Option<String>,
    pub s3: Option<S3Config>,
}

impl Config {
    pub fn from_env() -> Result<Self, StartupError> {
        let data_dir = env::var("IMMICH_DATA_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data"));
        let host = match optional_env("IMMICH_HOST") {
            Some(value) => value.parse::<IpAddr>().map_err(|_| StartupError::Invalid {
                name: "IMMICH_HOST",
            })?,
            None => IpAddr::from([0, 0, 0, 0]),
        };
        let port = match optional_env("IMMICH_PORT") {
            Some(value) => value.parse::<u16>().map_err(|_| StartupError::Invalid {
                name: "IMMICH_PORT",
            })?,
            None => 2283,
        };
        let web_root = env::var("IMMICH_WEB_ROOT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("web"));
        Ok(Self {
            data_dir,
            web_root,
            host,
            port,
            admin_email: optional_env("IMMICH_ADMIN_EMAIL"),
            admin_name: optional_env("IMMICH_ADMIN_NAME"),
            admin_password: optional_env("IMMICH_ADMIN_PASSWORD"),
            s3: s3_from_env()?,
        })
    }

    /// Names of the admin values that are absent. Startup refuses these only when no User exists.
    pub fn missing_admin(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.admin_email.is_none() {
            names.push("IMMICH_ADMIN_EMAIL");
        }
        if self.admin_name.is_none() {
            names.push("IMMICH_ADMIN_NAME");
        }
        if self.admin_password.is_none() {
            names.push("IMMICH_ADMIN_PASSWORD");
        }
        names
    }
}

fn s3_from_env() -> Result<Option<S3Config>, StartupError> {
    let endpoint = optional_env("IMMICH_S3_ENDPOINT");
    let bucket = optional_env("IMMICH_S3_BUCKET");
    let access_key_id = optional_env("IMMICH_S3_ACCESS_KEY_ID");
    let secret_access_key = optional_env("IMMICH_S3_SECRET_ACCESS_KEY");
    let any = endpoint.is_some()
        || bucket.is_some()
        || access_key_id.is_some()
        || secret_access_key.is_some();
    if !any {
        return Ok(None);
    }
    let mut missing = Vec::new();
    if endpoint.is_none() {
        missing.push("IMMICH_S3_ENDPOINT");
    }
    if bucket.is_none() {
        missing.push("IMMICH_S3_BUCKET");
    }
    if access_key_id.is_none() {
        missing.push("IMMICH_S3_ACCESS_KEY_ID");
    }
    if secret_access_key.is_none() {
        missing.push("IMMICH_S3_SECRET_ACCESS_KEY");
    }
    if !missing.is_empty() {
        return Err(StartupError::MissingConfig { names: missing });
    }
    let path_style = match optional_env("IMMICH_S3_PATH_STYLE") {
        Some(value) => !matches!(value.as_str(), "0" | "false" | "FALSE"),
        None => true,
    };
    Ok(Some(S3Config {
        endpoint: endpoint.expect("checked"),
        bucket: bucket.expect("checked"),
        access_key_id: access_key_id.expect("checked"),
        secret_access_key: secret_access_key.expect("checked"),
        region: optional_env("IMMICH_S3_REGION").unwrap_or_else(|| "us-east-1".to_string()),
        path_style,
    }))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug)]
pub enum StartupError {
    MissingConfig { names: Vec<&'static str> },
    Invalid { name: &'static str },
    Database(sqlx::Error),
    Io(std::io::Error),
    Hash(bcrypt::BcryptError),
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfig { names } => write!(formatter, "missing {}", names.join(", ")),
            Self::Invalid { name } => write!(formatter, "invalid {name}"),
            Self::Database(error) => write!(formatter, "database: {error}"),
            Self::Io(error) => write!(formatter, "io: {error}"),
            Self::Hash(error) => write!(formatter, "password hash: {error}"),
        }
    }
}

impl std::error::Error for StartupError {}
