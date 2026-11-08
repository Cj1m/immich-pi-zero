use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::config::{Config, StartupError};

const AVATAR_COLORS: [&str; 10] = [
    "primary", "pink", "red", "yellow", "blue", "green", "purple", "orange", "gray", "amber",
];

/// Open database and the in-memory pieces a request needs.
#[derive(Clone)]
pub struct App {
    pool: SqlitePool,
    dummy_password_hash: String,
    store: std::sync::Arc<crate::store::Store>,
    data_dir: std::path::PathBuf,
    web_root: std::path::PathBuf,
    machine_learning_url: String,
    machine_learning_enabled: bool,
    config_file_locked: bool,
    log_level_locked: bool,
}

impl App {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) fn store(&self) -> &crate::store::Store {
        &self.store
    }

    pub(crate) fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub(crate) fn web_root(&self) -> &std::path::Path {
        &self.web_root
    }

    pub(crate) fn machine_learning_url(&self) -> &str {
        &self.machine_learning_url
    }

    pub(crate) fn machine_learning_enabled(&self) -> bool {
        self.machine_learning_enabled
    }

    pub(crate) fn config_file_locked(&self) -> bool {
        self.config_file_locked
    }

    pub(crate) fn log_level_locked(&self) -> bool {
        self.log_level_locked
    }

    pub(crate) async fn quota_size(&self, user_id: &str) -> Result<Option<i64>, DbError> {
        let value: Option<Option<i64>> =
            sqlx::query_scalar("SELECT quotaSizeInBytes FROM user WHERE id = ?")
                .bind(user_id)
                .fetch_optional(self.pool())
                .await?;
        Ok(value.flatten())
    }

    pub async fn prepare(config: &Config) -> Result<Self, StartupError> {
        std::fs::create_dir_all(&config.data_dir).map_err(StartupError::Io)?;
        let database_path = config.data_dir.join("immich.db");
        let pool = connect(&database_path).await?;
        migrate(&pool).await?;
        let user_count = user_count(&pool).await?;
        if user_count == 0 {
            let missing = config.missing_admin();
            if !missing.is_empty() {
                return Err(StartupError::MissingConfig { names: missing });
            }
            create_first_admin(
                &pool,
                config.admin_email.as_deref().expect("email checked"),
                config.admin_name.as_deref().expect("name checked"),
                config.admin_password.as_deref().expect("password checked"),
            )
            .await?;
        }
        ensure_admin_onboarding(&pool).await?;
        if let Some(path) = &config.config_file {
            crate::system_config::install_config_file(&pool, path).await?;
        }
        let dummy_password_hash =
            bcrypt::hash("immich-login-dummy", 10).map_err(StartupError::Hash)?;
        let store = crate::store::Store::open(config).map_err(StartupError::Io)?;
        let app = Self {
            pool,
            dummy_password_hash,
            store: std::sync::Arc::new(store),
            data_dir: config.data_dir.clone(),
            web_root: config.web_root.clone(),
            machine_learning_url: config.machine_learning_url.clone(),
            machine_learning_enabled: config.machine_learning_enabled,
            config_file_locked: config.config_file.is_some(),
            log_level_locked: config.log_level.is_some(),
        };
        if let Err(error) = crate::ml::discard_mismatched_clip(&app).await {
            eprintln!("clip: {error}");
        }
        Ok(app)
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
        device: &ClientDevice,
    ) -> Result<LoginSuccess, DbError> {
        let email = email.to_lowercase();
        let user = find_active_user_by_email(&self.pool, &email).await?;
        let hash = user
            .as_ref()
            .and_then(|user| user.password.as_deref())
            .unwrap_or(self.dummy_password_hash.as_str());
        let matches = bcrypt::verify(password, hash).unwrap_or(false);
        let Some(user) = user else {
            return Err(DbError::InvalidCredentials);
        };
        if user.password.is_none() || !matches {
            return Err(DbError::InvalidCredentials);
        }
        let token = random_token();
        let token_hash = sha256(token.as_bytes());
        let now = now_iso();
        let session_id = Uuid::new_v4().to_string();
        let update_id = new_update_id();
        sqlx::query(
            "INSERT INTO session (
                id, token, createdAt, updatedAt, expiresAt, userId, parentId,
                deviceType, deviceOS, appVersion, updateId, isPendingSyncReset,
                pinExpiresAt, oauthSid, oauthBearerToken
            ) VALUES (?, ?, ?, ?, NULL, ?, NULL, ?, ?, ?, ?, 0, NULL, NULL, NULL)",
        )
        .bind(&session_id)
        .bind(&token_hash)
        .bind(&now)
        .bind(&now)
        .bind(&user.id)
        .bind(&device.device_type)
        .bind(&device.device_os)
        .bind(&device.app_version)
        .bind(&update_id)
        .execute(&self.pool)
        .await?;
        let onboarded = user_is_onboarded(&self.pool, &user.id).await?;
        Ok(LoginSuccess {
            token,
            user: user.into_document(onboarded),
        })
    }

    pub async fn session_user(&self, token: &str) -> Result<CurrentSession, DbError> {
        let token_hash = sha256(token.as_bytes());
        let row = sqlx::query_as::<_, SessionUser>(SESSION_USER_SQL)
            .bind(token_hash.as_slice())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(DbError::InvalidToken);
        };
        if row.deleted_at.is_some() {
            return Err(DbError::InvalidToken);
        }
        let onboarded = user_is_onboarded(&self.pool, &row.id).await?;
        Ok(CurrentSession {
            session_id: row.session_id.clone(),
            updated_at: row.session_updated_at.clone(),
            app_version: row.session_app_version.clone(),
            user: row.into_user(onboarded),
        })
    }

    /// Server onboarding starts true, so the storage wizard does not run.
    pub async fn admin_onboarded(&self) -> bool {
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM system_metadata WHERE key = 'admin-onboarding'")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        let Some(value) = value else {
            return true;
        };
        serde_json::from_str::<serde_json::Value>(&value)
            .ok()
            .and_then(|json| json.get("isOnboarded").and_then(|flag| flag.as_bool()))
            .unwrap_or(true)
    }

    pub async fn logout(&self, session_id: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM session WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn touch_session(
        &self,
        session_id: &str,
        device: &ClientDevice,
        stored_updated_at: &str,
        stored_app_version: Option<&str>,
    ) -> Result<(), DbError> {
        let version_changed = device.app_version.as_deref() != stored_app_version;
        let stale = age_hours(stored_updated_at) > 1.0;
        if !version_changed && !stale {
            return Ok(());
        }
        sqlx::query(
            "UPDATE session
             SET updatedAt = ?, appVersion = ?, deviceOS = ?, deviceType = ?, updateId = ?
             WHERE id = ?",
        )
        .bind(now_iso())
        .bind(&device.app_version)
        .bind(&device.device_os)
        .bind(&device.device_type)
        .bind(new_update_id())
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

const SESSION_USER_SQL: &str = "SELECT
    session.id AS session_id,
    session.updatedAt AS session_updated_at,
    session.appVersion AS session_app_version,
    user.id, user.email, user.name, user.profileImagePath, user.avatarColor,
    user.profileChangedAt, user.clusterGroupId, user.storageLabel,
    user.shouldChangePassword, user.isAdmin, user.createdAt, user.deletedAt,
    user.updatedAt, user.oauthId, user.quotaSizeInBytes, user.quotaUsageInBytes,
    user.status
 FROM session
 JOIN user ON user.id = session.userId
 WHERE session.token = ?";

#[derive(Debug)]
pub struct LoginSuccess {
    pub token: String,
    pub user: UserDocument,
}

#[derive(Debug)]
pub struct CurrentSession {
    pub session_id: String,
    pub updated_at: String,
    pub app_version: Option<String>,
    pub user: UserDocument,
}

/// What the official clients send about the device that signed in.
#[derive(Debug, Clone)]
pub struct ClientDevice {
    pub device_type: String,
    pub device_os: String,
    pub app_version: Option<String>,
}

#[derive(Debug)]
pub struct UserDocument {
    pub id: String,
    pub email: String,
    pub name: String,
    pub profile_image_path: String,
    pub avatar_color: String,
    pub profile_changed_at: String,
    pub cluster_group_id: Option<String>,
    pub storage_label: Option<String>,
    pub should_change_password: bool,
    pub is_admin: bool,
    pub created_at: String,
    pub deleted_at: Option<String>,
    pub updated_at: String,
    pub oauth_id: String,
    pub quota_size_in_bytes: Option<i64>,
    pub quota_usage_in_bytes: i64,
    pub status: String,
    pub is_onboarded: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct UserRow {
    id: String,
    email: String,
    password: Option<String>,
    name: String,
    profile_image_path: String,
    avatar_color: Option<String>,
    profile_changed_at: String,
    cluster_group_id: Option<String>,
    storage_label: Option<String>,
    should_change_password: i64,
    is_admin: i64,
    created_at: String,
    deleted_at: Option<String>,
    updated_at: String,
    oauth_id: Option<String>,
    quota_size_in_bytes: Option<i64>,
    quota_usage_in_bytes: i64,
    status: String,
}

impl UserRow {
    fn into_document(self, is_onboarded: bool) -> UserDocument {
        UserDocument {
            id: self.id,
            email: self.email.clone(),
            name: self.name,
            profile_image_path: self.profile_image_path,
            avatar_color: self
                .avatar_color
                .unwrap_or_else(|| avatar_color_for_email(&self.email).to_string()),
            profile_changed_at: self.profile_changed_at,
            cluster_group_id: self.cluster_group_id,
            storage_label: self.storage_label,
            should_change_password: self.should_change_password != 0,
            is_admin: self.is_admin != 0,
            created_at: self.created_at,
            deleted_at: self.deleted_at,
            updated_at: self.updated_at,
            oauth_id: self.oauth_id.unwrap_or_default(),
            quota_size_in_bytes: self.quota_size_in_bytes,
            quota_usage_in_bytes: self.quota_usage_in_bytes,
            status: self.status,
            is_onboarded,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct SessionUser {
    session_id: String,
    session_updated_at: String,
    session_app_version: Option<String>,
    id: String,
    email: String,
    name: String,
    #[sqlx(rename = "profileImagePath")]
    profile_image_path: String,
    #[sqlx(rename = "avatarColor")]
    avatar_color: Option<String>,
    #[sqlx(rename = "profileChangedAt")]
    profile_changed_at: String,
    #[sqlx(rename = "clusterGroupId")]
    cluster_group_id: Option<String>,
    #[sqlx(rename = "storageLabel")]
    storage_label: Option<String>,
    #[sqlx(rename = "shouldChangePassword")]
    should_change_password: i64,
    #[sqlx(rename = "isAdmin")]
    is_admin: i64,
    #[sqlx(rename = "createdAt")]
    created_at: String,
    #[sqlx(rename = "deletedAt")]
    deleted_at: Option<String>,
    #[sqlx(rename = "updatedAt")]
    updated_at: String,
    #[sqlx(rename = "oauthId")]
    oauth_id: Option<String>,
    #[sqlx(rename = "quotaSizeInBytes")]
    quota_size_in_bytes: Option<i64>,
    #[sqlx(rename = "quotaUsageInBytes")]
    quota_usage_in_bytes: i64,
    status: String,
}

impl SessionUser {
    fn into_user(self, is_onboarded: bool) -> UserDocument {
        UserDocument {
            id: self.id,
            email: self.email.clone(),
            name: self.name,
            profile_image_path: self.profile_image_path,
            avatar_color: self
                .avatar_color
                .unwrap_or_else(|| avatar_color_for_email(&self.email).to_string()),
            profile_changed_at: self.profile_changed_at,
            cluster_group_id: self.cluster_group_id,
            storage_label: self.storage_label,
            should_change_password: self.should_change_password != 0,
            is_admin: self.is_admin != 0,
            created_at: self.created_at,
            deleted_at: self.deleted_at,
            updated_at: self.updated_at,
            oauth_id: self.oauth_id.unwrap_or_default(),
            quota_size_in_bytes: self.quota_size_in_bytes,
            quota_usage_in_bytes: self.quota_usage_in_bytes,
            status: self.status,
            is_onboarded,
        }
    }
}

#[derive(Debug)]
pub enum DbError {
    InvalidCredentials,
    InvalidToken,
    NotFound,
    Validation,
    Quota,
    Database(sqlx::Error),
    Store(String),
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<crate::store::StoreError> for DbError {
    fn from(error: crate::store::StoreError) -> Self {
        Self::Store(error.0)
    }
}

async fn connect(path: &Path) -> Result<SqlitePool, StartupError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .map_err(StartupError::Database)
}

async fn migrate(pool: &SqlitePool) -> Result<(), StartupError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_migration (
            version INTEGER PRIMARY KEY NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(StartupError::Database)?;
    let current: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_migration")
        .fetch_one(pool)
        .await
        .map_err(StartupError::Database)?;
    if current < 1 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V1.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (1)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    if current < 2 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V2.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        let user_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM user")
            .fetch_all(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        for user_id in user_ids {
            sqlx::query("UPDATE user SET updateId = ? WHERE id = ?")
                .bind(new_update_id())
                .bind(user_id)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (2)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    if current < 3 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V3.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (3)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    if current < 4 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V4.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (4)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    if current < 5 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V5.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (5)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    if current < 6 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V6.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (6)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    if current < 7 {
        let mut transaction = pool.begin().await.map_err(StartupError::Database)?;
        for statement in SCHEMA_V7.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .map_err(StartupError::Database)?;
        }
        sqlx::query("INSERT INTO schema_migration (version) VALUES (7)")
            .execute(&mut *transaction)
            .await
            .map_err(StartupError::Database)?;
        transaction.commit().await.map_err(StartupError::Database)?;
    }
    Ok(())
}

const SCHEMA_V1: &str = "
CREATE TABLE user (
    id TEXT PRIMARY KEY NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password TEXT,
    pinCode TEXT,
    createdAt TEXT NOT NULL,
    profileImagePath TEXT NOT NULL DEFAULT '',
    isAdmin INTEGER NOT NULL DEFAULT 0,
    shouldChangePassword INTEGER NOT NULL DEFAULT 1,
    avatarColor TEXT,
    deletedAt TEXT,
    oauthId TEXT,
    updatedAt TEXT NOT NULL,
    storageLabel TEXT UNIQUE,
    name TEXT NOT NULL DEFAULT '',
    quotaSizeInBytes INTEGER,
    quotaUsageInBytes INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    profileChangedAt TEXT NOT NULL,
    updateId TEXT NOT NULL,
    clusterGroupId TEXT
);
CREATE TABLE session (
    id TEXT PRIMARY KEY NOT NULL,
    token BLOB NOT NULL,
    createdAt TEXT NOT NULL,
    updatedAt TEXT NOT NULL,
    expiresAt TEXT,
    userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    parentId TEXT REFERENCES session(id) ON DELETE CASCADE,
    deviceType TEXT NOT NULL DEFAULT '',
    deviceOS TEXT NOT NULL DEFAULT '',
    appVersion TEXT,
    updateId TEXT NOT NULL,
    isPendingSyncReset INTEGER NOT NULL DEFAULT 0,
    pinExpiresAt TEXT,
    oauthSid TEXT,
    oauthBearerToken TEXT
);
CREATE INDEX session_token ON session(token);
CREATE TABLE user_metadata (
    userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updateId TEXT NOT NULL,
    updatedAt TEXT NOT NULL,
    PRIMARY KEY (userId, key)
);
CREATE TABLE system_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
";

const SCHEMA_V2: &str = "
CREATE TABLE asset (
    id TEXT PRIMARY KEY NOT NULL,
    ownerId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    originalPath TEXT NOT NULL,
    originalFileName TEXT NOT NULL,
    originalMimeType TEXT NOT NULL,
    fileCreatedAt TEXT NOT NULL,
    fileModifiedAt TEXT NOT NULL,
    localDateTime TEXT NOT NULL,
    isFavorite INTEGER NOT NULL DEFAULT 0,
    duration INTEGER,
    checksum BLOB NOT NULL,
    checksumAlgorithm TEXT NOT NULL DEFAULT 'sha1',
    livePhotoVideoId TEXT,
    thumbhash BLOB,
    isOffline INTEGER NOT NULL DEFAULT 0,
    libraryId TEXT,
    isExternal INTEGER NOT NULL DEFAULT 0,
    deletedAt TEXT,
    stackId TEXT,
    duplicateId TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    updateId TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'timeline',
    width INTEGER,
    height INTEGER,
    isEdited INTEGER NOT NULL DEFAULT 0,
    createdAt TEXT NOT NULL,
    updatedAt TEXT NOT NULL
);
CREATE UNIQUE INDEX asset_owner_checksum ON asset(ownerId, checksum);
CREATE INDEX asset_owner_update ON asset(ownerId, updateId);
CREATE TABLE asset_exif (
    assetId TEXT PRIMARY KEY NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    make TEXT,
    model TEXT,
    exifImageWidth INTEGER,
    exifImageHeight INTEGER,
    fileSizeInByte INTEGER,
    orientation TEXT,
    dateTimeOriginal TEXT,
    modifyDate TEXT,
    lensModel TEXT,
    fNumber REAL,
    focalLength REAL,
    iso INTEGER,
    latitude REAL,
    longitude REAL,
    city TEXT,
    state TEXT,
    country TEXT,
    description TEXT NOT NULL DEFAULT '',
    fps REAL,
    exposureTime TEXT,
    livePhotoCID TEXT,
    timeZone TEXT,
    projectionType TEXT,
    profileDescription TEXT,
    colorspace TEXT,
    bitsPerSample INTEGER,
    autoStackId TEXT,
    rating INTEGER,
    tags TEXT,
    updatedAt TEXT NOT NULL,
    updateId TEXT NOT NULL,
    lockedProperties TEXT
);
CREATE TABLE asset_job_status (
    assetId TEXT PRIMARY KEY NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    facesRecognizedAt TEXT,
    metadataExtractedAt TEXT,
    duplicatesDetectedAt TEXT,
    ocrAt TEXT
);
CREATE TABLE asset_file (
    id TEXT PRIMARY KEY NOT NULL,
    assetId TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    path TEXT NOT NULL,
    UNIQUE(assetId, type)
);
CREATE TABLE asset_audit (
    id TEXT PRIMARY KEY NOT NULL,
    assetId TEXT NOT NULL,
    ownerId TEXT NOT NULL,
    updateId TEXT NOT NULL
);
CREATE INDEX asset_audit_owner ON asset_audit(ownerId, updateId);
CREATE TABLE album (
    id TEXT PRIMARY KEY NOT NULL,
    ownerId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    albumName TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    createdAt TEXT NOT NULL,
    updatedAt TEXT NOT NULL,
    albumThumbnailAssetId TEXT,
    isActivityEnabled INTEGER NOT NULL DEFAULT 1,
    albumOrder TEXT NOT NULL DEFAULT 'desc',
    updateId TEXT NOT NULL
);
CREATE TABLE album_user (
    albumId TEXT NOT NULL REFERENCES album(id) ON DELETE CASCADE,
    userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    updateId TEXT NOT NULL,
    PRIMARY KEY (albumId, userId)
);
CREATE TABLE album_asset (
    albumId TEXT NOT NULL REFERENCES album(id) ON DELETE CASCADE,
    assetId TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    createdAt TEXT NOT NULL,
    updateId TEXT NOT NULL,
    PRIMARY KEY (albumId, assetId)
);
CREATE TABLE album_audit (
    id TEXT PRIMARY KEY NOT NULL,
    albumId TEXT NOT NULL,
    ownerId TEXT NOT NULL,
    updateId TEXT NOT NULL
);
CREATE TABLE album_asset_audit (
    id TEXT PRIMARY KEY NOT NULL,
    albumId TEXT NOT NULL,
    assetId TEXT NOT NULL,
    ownerId TEXT NOT NULL,
    updateId TEXT NOT NULL
);
CREATE TABLE album_user_audit (
    id TEXT PRIMARY KEY NOT NULL,
    albumId TEXT NOT NULL,
    userId TEXT NOT NULL,
    ownerId TEXT NOT NULL,
    updateId TEXT NOT NULL
);
CREATE TABLE sync_checkpoint (
    sessionId TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    ack TEXT NOT NULL,
    PRIMARY KEY (sessionId, type)
);
CREATE TABLE stack (
    id TEXT PRIMARY KEY NOT NULL,
    ownerId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    primaryAssetId TEXT NOT NULL,
    createdAt TEXT NOT NULL,
    updatedAt TEXT NOT NULL,
    updateId TEXT NOT NULL
);
";

const SCHEMA_V3: &str = "
CREATE TABLE shared_link (
    id TEXT PRIMARY KEY NOT NULL,
    description TEXT,
    userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    key BLOB NOT NULL UNIQUE,
    type TEXT NOT NULL,
    createdAt TEXT NOT NULL,
    expiresAt TEXT,
    allowUpload INTEGER NOT NULL,
    albumId TEXT REFERENCES album(id) ON DELETE CASCADE,
    allowDownload INTEGER NOT NULL,
    showExif INTEGER NOT NULL,
    password TEXT,
    slug TEXT
);
CREATE UNIQUE INDEX shared_link_slug_uq ON shared_link(slug) WHERE slug IS NOT NULL;
CREATE TABLE shared_link_asset (
    assetId TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    sharedLinkId TEXT NOT NULL REFERENCES shared_link(id) ON DELETE CASCADE,
    PRIMARY KEY (sharedLinkId, assetId)
);
";

const SCHEMA_V4: &str = "
CREATE TABLE asset_clip (
    assetId TEXT PRIMARY KEY NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    embedding BLOB NOT NULL
);
";

const SCHEMA_V5: &str = "
CREATE TABLE memory (
    id TEXT PRIMARY KEY NOT NULL,
    createdAt TEXT NOT NULL,
    updatedAt TEXT NOT NULL,
    deletedAt TEXT,
    ownerId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    data TEXT NOT NULL,
    isSaved INTEGER NOT NULL DEFAULT 0,
    memoryAt TEXT NOT NULL,
    seenAt TEXT,
    showAt TEXT,
    hideAt TEXT,
    updateId TEXT NOT NULL
);
CREATE INDEX memory_owner ON memory(ownerId, updateId);
CREATE TABLE memory_asset (
    memoryId TEXT NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
    assetId TEXT NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    updateId TEXT NOT NULL,
    PRIMARY KEY (memoryId, assetId)
);
CREATE TABLE memory_audit (
    id TEXT PRIMARY KEY NOT NULL,
    memoryId TEXT NOT NULL,
    ownerId TEXT NOT NULL,
    updateId TEXT NOT NULL
);
CREATE INDEX memory_audit_owner ON memory_audit(ownerId, updateId);
CREATE TABLE memory_asset_audit (
    id TEXT PRIMARY KEY NOT NULL,
    memoryId TEXT NOT NULL,
    assetId TEXT NOT NULL,
    ownerId TEXT NOT NULL,
    updateId TEXT NOT NULL
);
CREATE INDEX memory_asset_audit_owner ON memory_asset_audit(ownerId, updateId);
";

const SCHEMA_V6: &str = "
CREATE TABLE activity (
    id TEXT PRIMARY KEY NOT NULL,
    createdAt TEXT NOT NULL,
    updatedAt TEXT NOT NULL,
    albumId TEXT NOT NULL REFERENCES album(id) ON DELETE CASCADE,
    userId TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    assetId TEXT REFERENCES asset(id) ON DELETE CASCADE,
    comment TEXT,
    isLiked INTEGER NOT NULL,
    updateId TEXT NOT NULL
);
CREATE INDEX activity_album ON activity(albumId, createdAt);
";

const SCHEMA_V7: &str = "
CREATE TABLE geodata_places (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    alternateNames TEXT,
    latitude REAL NOT NULL,
    longitude REAL NOT NULL,
    countryCode TEXT,
    admin1Code TEXT,
    admin2Code TEXT,
    admin1Name TEXT,
    admin2Name TEXT,
    modificationDate TEXT
);
";

async fn user_count(pool: &SqlitePool) -> Result<i64, StartupError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM user")
        .fetch_one(pool)
        .await
        .map_err(StartupError::Database)
}

async fn create_first_admin(
    pool: &SqlitePool,
    email: &str,
    name: &str,
    password: &str,
) -> Result<(), StartupError> {
    let now = now_iso();
    let id = Uuid::new_v4().to_string();
    let update_id = new_update_id();
    let password_hash = bcrypt::hash(password, 10).map_err(StartupError::Hash)?;
    sqlx::query(
        "INSERT INTO user (
            id, email, password, pinCode, createdAt, profileImagePath, isAdmin,
            shouldChangePassword, avatarColor, deletedAt, oauthId, updatedAt,
            storageLabel, name, quotaSizeInBytes, quotaUsageInBytes, status,
            profileChangedAt, updateId, clusterGroupId
        ) VALUES (
            ?, ?, ?, NULL, ?, '', 1,
            1, NULL, NULL, NULL, ?,
            'admin', ?, NULL, 0, 'active',
            ?, ?, NULL
        )",
    )
    .bind(&id)
    .bind(email.trim().to_lowercase())
    .bind(password_hash)
    .bind(&now)
    .bind(&now)
    .bind(name)
    .bind(&now)
    .bind(&update_id)
    .execute(pool)
    .await
    .map_err(StartupError::Database)?;
    Ok(())
}

async fn ensure_admin_onboarding(pool: &SqlitePool) -> Result<(), StartupError> {
    sqlx::query(
        "INSERT OR IGNORE INTO system_metadata (key, value)
         VALUES ('admin-onboarding', '{\"isOnboarded\":true}')",
    )
    .execute(pool)
    .await
    .map_err(StartupError::Database)?;
    Ok(())
}

async fn find_active_user_by_email(
    pool: &SqlitePool,
    email: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT
            id, email, password, name,
            profileImagePath AS profile_image_path,
            avatarColor AS avatar_color,
            profileChangedAt AS profile_changed_at,
            clusterGroupId AS cluster_group_id,
            storageLabel AS storage_label,
            shouldChangePassword AS should_change_password,
            isAdmin AS is_admin,
            createdAt AS created_at,
            deletedAt AS deleted_at,
            updatedAt AS updated_at,
            oauthId AS oauth_id,
            quotaSizeInBytes AS quota_size_in_bytes,
            quotaUsageInBytes AS quota_usage_in_bytes,
            status
         FROM user
         WHERE email = ? AND deletedAt IS NULL",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
}

async fn user_is_onboarded(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let value: Option<String> = sqlx::query_scalar(
        "SELECT value FROM user_metadata WHERE userId = ? AND key = 'onboarding'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(value) = value else {
        return Ok(false);
    };
    Ok(serde_json::from_str::<serde_json::Value>(&value)
        .ok()
        .and_then(|json| json.get("isOnboarded").and_then(|flag| flag.as_bool()))
        .unwrap_or(false))
}

pub(crate) fn avatar_color_for_email(email: &str) -> &'static str {
    let sum: u32 = email.chars().map(|letter| letter as u32).sum();
    AVATAR_COLORS[(sum as usize) % AVATAR_COLORS.len()]
}

pub(crate) fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn new_update_id() -> String {
    Uuid::now_v7().to_string()
}

pub(crate) fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn age_hours(timestamp: &str) -> f64 {
    let Ok(then) = DateTime::parse_from_rfc3339(timestamp) else {
        return f64::MAX;
    };
    let seconds = (Utc::now() - then.with_timezone(&Utc)).num_seconds();
    seconds as f64 / 3600.0
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn random_token() -> String {
    use base64::Engine;
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("os random");
    base64::engine::general_purpose::STANDARD
        .encode(bytes)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect()
}
