use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::TimeZone;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::{App, ClientDevice, DbError, UserDocument};

const COOKIE_MAX_AGE_SECONDS: u64 = 400 * 24 * 60 * 60;
const ARCHIVE_SIZE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub fn router(app: App) -> Router {
    Router::new()
        .route("/.well-known/immich", get(well_known))
        .route("/api/server/ping", get(ping))
        .route("/api/server/version", get(version))
        .route("/api/server/features", get(features))
        .route("/api/server/config", get(server_config))
        .route("/api/auth/login", post(login))
        .route("/api/auth/validateToken", post(validate_token))
        .route("/api/auth/logout", post(logout))
        .route("/api/users/me", get(current_user))
        .route("/api/users/me/preferences", get(preferences))
        .merge(crate::http_media::routes())
        .fallback(not_found)
        .layer(DefaultBodyLimit::disable())
        .with_state(app)
}

async fn well_known() -> Response {
    json_ok(json!({ "api": { "endpoint": "/api" } }))
}

async fn ping() -> Response {
    json_ok(json!({ "res": "pong" }))
}

async fn version() -> Response {
    json_ok(json!({
        "major": 3,
        "minor": 3,
        "patch": 6,
        "prerelease": null,
    }))
}

async fn features() -> Response {
    // `email` stays false (no mail is sent) and `realtimeTranscoding` stays false (no encoded video).
    json_ok(json!({
        "smartSearch": true,
        "duplicateDetection": true,
        "configFile": false,
        "facialRecognition": true,
        "map": true,
        "trash": true,
        "reverseGeocoding": true,
        "importFaces": false,
        "oauth": false,
        "oauthAutoLaunch": false,
        "passwordLogin": true,
        "sidecar": true,
        "search": true,
        "email": false,
        "ocr": true,
        "realtimeTranscoding": false,
    }))
}

async fn server_config(State(app): State<App>) -> Response {
    let is_onboarded = app.admin_onboarded().await;
    json_ok(json!({
        "loginPageMessage": "",
        "trashDays": 30,
        "userDeleteDelay": 7,
        "oauthButtonText": "Login with OAuth",
        "oauthAccountManagementUrl": "",
        "isInitialized": true,
        "isOnboarded": is_onboarded,
        "externalDomain": "",
        "publicUsers": true,
        "mapDarkStyleUrl": "https://tiles.immich.cloud/v1/style/dark.json",
        "mapLightStyleUrl": "https://tiles.immich.cloud/v1/style/light.json",
        "maintenanceMode": false,
        "minFaces": 3,
    }))
}

async fn login(State(app): State<App>, request: Request) -> Response {
    let secure = request_is_secure(&request);
    let headers = request.headers().clone();
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 64 * 1024).await else {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    };
    let credentials: LoginBody = match serde_json::from_slice(&bytes) {
        Ok(credentials) => credentials,
        Err(_) => return message(StatusCode::BAD_REQUEST, "Validation failed"),
    };
    if !valid_email(&credentials.email) {
        return message(StatusCode::BAD_REQUEST, "Validation failed");
    }
    let device = client_device(&headers);
    match app
        .login(&credentials.email, &credentials.password, &device)
        .await
    {
        Ok(success) => login_response(success.token, &success.user, secure),
        Err(error) => db_error(error),
    }
}

async fn validate_token(State(app): State<App>, request: Request) -> Response {
    if let Err(response) = require_session(&app, request.headers()).await {
        return response;
    }
    json_ok(json!({ "authStatus": true }))
}

async fn logout(State(app): State<App>, request: Request) -> Response {
    let session = match require_session(&app, request.headers()).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(error) = app.logout(&session.session_id).await {
        return db_error(error);
    }
    let mut response = json_ok(json!({
        "successful": true,
        "redirectUri": "/auth/login?autoLaunch=0",
    }));
    let headers = response.headers_mut();
    for cookie in [
        "immich_access_token",
        "immich_auth_type",
        "immich_is_authenticated",
    ] {
        headers.append(
            header::SET_COOKIE,
            format!("{cookie}=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT")
                .parse()
                .expect("clear cookie"),
        );
    }
    response
}

async fn current_user(State(app): State<App>, request: Request) -> Response {
    let session = match require_session(&app, request.headers()).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    json_ok(user_json(&session.user))
}

async fn preferences(State(app): State<App>, request: Request) -> Response {
    if let Err(response) = require_session(&app, request.headers()).await {
        return response;
    }
    json_ok(default_preferences())
}

async fn not_found(request: Request) -> Response {
    let method = request.method();
    let path = request.uri().path();
    message(StatusCode::NOT_FOUND, format!("Cannot {method} {path}"))
}

#[derive(Deserialize)]
struct LoginBody {
    email: String,
    password: String,
}

pub(crate) async fn require_session(
    app: &App,
    headers: &HeaderMap,
) -> Result<crate::db::CurrentSession, Response> {
    // A Session is accepted only from the immich_access_token cookie.
    let Some(token) = cookie(headers, "immich_access_token") else {
        return Err(message(StatusCode::UNAUTHORIZED, "Authentication required"));
    };
    let session = match app.session_user(&token).await {
        Ok(session) => session,
        Err(error) => return Err(db_error(error)),
    };
    let device = client_device(headers);
    if let Err(error) = app
        .touch_session(
            &session.session_id,
            &device,
            &session.updated_at,
            session.app_version.as_deref(),
        )
        .await
    {
        return Err(db_error(error));
    }
    Ok(session)
}

fn login_response(token: String, user: &UserDocument, secure: bool) -> Response {
    let mut response = (
        StatusCode::CREATED,
        axum::Json(json!({
            "accessToken": token,
            "userId": user.id,
            "userEmail": user.email,
            "name": user.name,
            "profileImagePath": user.profile_image_path,
            "isAdmin": user.is_admin,
            "shouldChangePassword": user.should_change_password,
            "isOnboarded": user.is_onboarded,
        })),
    )
        .into_response();
    let secure_flag = if secure { "; Secure" } else { "" };
    let common = format!("Max-Age={COOKIE_MAX_AGE_SECONDS}; Path=/; SameSite=Lax{secure_flag}");
    let headers = response.headers_mut();
    append_cookie(
        headers,
        format!("immich_access_token={token}; {common}; HttpOnly"),
    );
    append_cookie(
        headers,
        format!("immich_auth_type=password; {common}; HttpOnly"),
    );
    append_cookie(headers, format!("immich_is_authenticated=true; {common}"));
    response
}

pub(crate) fn user_json(user: &UserDocument) -> Value {
    json!({
        "id": user.id,
        "name": user.name,
        "email": user.email,
        "profileImagePath": user.profile_image_path,
        "avatarColor": user.avatar_color,
        "profileChangedAt": user.profile_changed_at,
        "clusterGroupId": user.cluster_group_id,
        "storageLabel": user.storage_label,
        "shouldChangePassword": user.should_change_password,
        "isAdmin": user.is_admin,
        "createdAt": user.created_at,
        "deletedAt": user.deleted_at,
        "updatedAt": user.updated_at,
        "oauthId": user.oauth_id,
        "quotaSizeInBytes": user.quota_size_in_bytes,
        "quotaUsageInBytes": user.quota_usage_in_bytes,
        "status": user.status,
        "license": null,
    })
}

fn default_preferences() -> Value {
    json!({
        "albums": { "defaultAssetOrder": "desc" },
        "folders": { "enabled": false, "sidebarWeb": false },
        "memories": { "enabled": true, "duration": 5, "sidebarWeb": false },
        "people": {
            "enabled": true,
            "sidebarWeb": false,
            "minimumFaces": 3,
            "updateStrategy": "everyone",
        },
        "sharedLinks": { "enabled": true, "sidebarWeb": false },
        "ratings": { "enabled": false },
        "tags": { "enabled": false, "sidebarWeb": false },
        "emailNotifications": { "enabled": true, "albumInvite": true, "albumUpdate": true },
        "download": {
            "archiveSize": ARCHIVE_SIZE_BYTES,
            "includeEmbeddedVideos": false,
        },
        "purchase": {
            "showSupportBadge": true,
            "hideBuyButtonUntil": local_date_iso(2022, 2, 12),
        },
        "cast": { "gCastEnabled": false },
        "recentlyAdded": { "sidebarWeb": false },
    })
}

fn local_date_iso(year: i32, month: u32, day: u32) -> String {
    chrono::Local
        .with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("local date")
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn client_device(headers: &HeaderMap) -> ClientDevice {
    let user_agent = header_str(headers, "user-agent").unwrap_or("");
    let app_version = immich_app_version(user_agent);
    let (browser, os) = browser_and_os(user_agent);
    let device_model = header_str(headers, "devicemodel").unwrap_or("");
    let device_header = header_str(headers, "devicetype").unwrap_or("");
    ClientDevice {
        device_type: if browser.is_empty() {
            device_model.to_string()
        } else {
            browser
        },
        device_os: if os.is_empty() {
            device_header.to_string()
        } else {
            os
        },
        app_version,
    }
}

fn immich_app_version(user_agent: &str) -> Option<String> {
    let rest = user_agent
        .strip_prefix("immich-android/")
        .or_else(|| user_agent.strip_prefix("immich-ios/"))
        .or_else(|| user_agent.strip_prefix("immich-unknown/"))
        .or_else(|| user_agent.strip_prefix("Immich_Android_"))
        .or_else(|| user_agent.strip_prefix("Immich_iOS_"))
        .or_else(|| user_agent.strip_prefix("Immich_Unknown_"))?;
    let version = rest.split_whitespace().next().unwrap_or(rest);
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

fn browser_and_os(user_agent: &str) -> (String, String) {
    let browser = if user_agent.contains("Edg/") {
        "Edge"
    } else if user_agent.contains("Chrome/") && !user_agent.contains("Chromium/") {
        "Chrome"
    } else if user_agent.contains("Firefox/") {
        "Firefox"
    } else if user_agent.contains("Safari/") && !user_agent.contains("Chrome/") {
        "Safari"
    } else {
        ""
    };
    let os = if user_agent.contains("Android") {
        "Android"
    } else if user_agent.contains("iPhone")
        || user_agent.contains("iPad")
        || user_agent.contains("iOS")
    {
        "iOS"
    } else if user_agent.contains("Mac OS X") {
        "Mac OS"
    } else if user_agent.contains("Windows") {
        "Windows"
    } else if user_agent.contains("Linux") {
        "Linux"
    } else {
        ""
    };
    (browser.to_string(), os.to_string())
}

fn request_is_secure(request: &Request<Body>) -> bool {
    if request.uri().scheme_str() == Some("https") {
        return true;
    }
    header_str(request.headers(), "x-forwarded-proto")
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in header.split(';') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !email.contains(char::is_whitespace)
        && !domain.contains('@')
}

fn append_cookie(headers: &mut HeaderMap, value: String) {
    headers.append(header::SET_COOKIE, value.parse().expect("set-cookie"));
}

pub(crate) fn json_ok(body: Value) -> Response {
    (StatusCode::OK, axum::Json(body)).into_response()
}

pub(crate) fn message(status: StatusCode, message: impl Into<String>) -> Response {
    (status, axum::Json(json!({ "message": message.into() }))).into_response()
}

pub(crate) fn db_error(error: DbError) -> Response {
    match error {
        DbError::InvalidCredentials => {
            message(StatusCode::UNAUTHORIZED, "Incorrect email or password")
        }
        DbError::InvalidToken => message(StatusCode::UNAUTHORIZED, "Invalid user token"),
        DbError::NotFound => message(StatusCode::NOT_FOUND, "Not found"),
        DbError::Validation => message(StatusCode::BAD_REQUEST, "Validation failed"),
        DbError::Quota => message(StatusCode::BAD_REQUEST, "Quota has been exceeded!"),
        DbError::Store(error) => {
            eprintln!("store: {error}");
            message(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
        DbError::Database(error) => internal(error),
    }
}

fn internal(error: sqlx::Error) -> Response {
    eprintln!("database: {error}");
    message(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}
