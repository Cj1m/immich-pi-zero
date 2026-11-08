use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use chrono::{Datelike, Timelike};
use http::Request;
use http_body_util::BodyExt;
use immich_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;

fn config(dir: &Path, password: Option<&str>) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        host: IpAddr::from([127, 0, 0, 1]),
        port: 0,
        admin_email: Some("Admin@Example.com".to_string()),
        admin_name: Some("Ada".to_string()),
        admin_password: password.map(str::to_string),
        s3: None,
    }
}

async fn prepare(dir: &Path, password: Option<&str>) -> App {
    App::prepare(&config(dir, password))
        .await
        .expect("server prepares")
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Value, Vec<String>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().expect("cookie").to_string())
        .collect();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json, cookies)
}

fn json_request(
    method: &str,
    path: &str,
    body: Option<&str>,
    cookie: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .expect("request")
}

#[tokio::test]
async fn startup_creates_the_admin_once() {
    let dir = std::env::temp_dir().join(format!("immich-startup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let error = match App::prepare(&config(&dir, None)).await {
        Err(error) => error,
        Ok(_) => panic!("missing password should refuse to start"),
    };
    let text = error.to_string();
    assert!(text.contains("IMMICH_ADMIN_PASSWORD"), "{text}");
    assert!(!text.contains("IMMICH_ADMIN_EMAIL"), "{text}");

    prepare(&dir, Some("correct horse")).await;
    let restarted = prepare(&dir, Some("a different password")).await;
    let (status, body, _) = call(
        &restarted,
        json_request(
            "POST",
            "/api/auth/login",
            Some(r#"{"email":"admin@example.com","password":"correct horse"}"#),
            None,
        ),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let (status, body, _) = call(
        &restarted,
        json_request(
            "POST",
            "/api/auth/login",
            Some(r#"{"email":"admin@example.com","password":"a different password"}"#),
            None,
        ),
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["message"], "Incorrect email or password");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn password_login_reaches_the_signed_in_user() {
    let dir = std::env::temp_dir().join(format!("immich-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = prepare(&dir, Some("correct horse")).await;

    let (status, body, _) =
        call(&app, json_request("GET", "/.well-known/immich", None, None)).await;
    assert_eq!(status, 200);
    assert_eq!(body["api"]["endpoint"], "/api");

    let (status, body, _) = call(&app, json_request("GET", "/api/server/ping", None, None)).await;
    assert_eq!(status, 200);
    assert_eq!(body["res"], "pong");

    let (status, body, _) =
        call(&app, json_request("GET", "/api/server/version", None, None)).await;
    assert_eq!(status, 200);
    assert_eq!(body["major"], 3);
    assert_eq!(body["minor"], 3);
    assert_eq!(body["patch"], 6);
    assert!(body["prerelease"].is_null());

    let (status, body, _) = call(
        &app,
        json_request("GET", "/api/server/features", None, None),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["passwordLogin"], true);
    assert_eq!(body["oauth"], false);
    assert_eq!(body["email"], false);
    assert_eq!(body["realtimeTranscoding"], false);
    assert_eq!(body["trash"], true);
    assert_eq!(body["ocr"], true);

    let (status, body, _) = call(&app, json_request("GET", "/api/server/config", None, None)).await;
    assert_eq!(status, 200);
    assert_eq!(body["isInitialized"], true);
    assert_eq!(body["isOnboarded"], true);
    assert_eq!(body["maintenanceMode"], false);
    assert_eq!(body["trashDays"], 30);
    assert_eq!(body["oauthButtonText"], "Login with OAuth");

    let (status, body, _) = call(
        &app,
        json_request("POST", "/api/auth/admin-sign-up", Some("{}"), None),
    )
    .await;
    assert_eq!(status, 404, "{body}");

    let (status, body, _) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/login",
            Some(r#"{"email":"not-an-email","password":"correct horse"}"#),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["message"], "Validation failed");

    let (status, body, cookies) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .header("user-agent", "immich-android/1.2.3")
            .header("devicemodel", "Pixel")
            .header("devicetype", "Android")
            .header("x-forwarded-proto", "https")
            .body(Body::from(
                r#"{"email":"Admin@Example.com","password":"correct horse"}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let token = body["accessToken"].as_str().expect("token").to_string();
    assert_eq!(body["userEmail"], "admin@example.com");
    assert_eq!(body["name"], "Ada");
    assert_eq!(body["isAdmin"], true);
    assert_eq!(body["shouldChangePassword"], true);
    assert_eq!(body["isOnboarded"], false);
    assert_eq!(body["profileImagePath"], "");
    assert!(cookies.iter().any(|cookie| {
        cookie.starts_with(&format!("immich_access_token={token};"))
            && cookie.contains("HttpOnly")
            && cookie.contains("Secure")
            && cookie.contains("Max-Age=34560000")
            && cookie.contains("Path=/")
            && cookie.contains("SameSite=Lax")
    }));
    assert!(cookies.iter().any(|cookie| {
        cookie.starts_with("immich_auth_type=password;") && cookie.contains("HttpOnly")
    }));
    assert!(cookies.iter().any(|cookie| {
        cookie.starts_with("immich_is_authenticated=true;") && !cookie.contains("HttpOnly")
    }));

    let session: (String, String, Option<String>) =
        sqlx::query_as("SELECT deviceType, deviceOS, appVersion FROM session WHERE token = ?")
            .bind(sha256(&token))
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(session.0, "Pixel");
    assert_eq!(session.1, "Android");
    assert_eq!(session.2.as_deref(), Some("1.2.3"));

    let cookie = format!("immich_access_token={token}");
    let (status, body, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", None, Some(&cookie)),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["authStatus"], true);

    let (status, _, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/validateToken")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 401);

    let (status, body, _) = call(
        &app,
        json_request("GET", "/api/users/me", None, Some(&cookie)),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["email"], "admin@example.com");
    assert_eq!(body["isAdmin"], true);
    assert_eq!(body["status"], "active");
    assert_eq!(body["storageLabel"], "admin");
    assert_eq!(body["oauthId"], "");
    assert!(body["clusterGroupId"].is_null());
    assert!(body["license"].is_null());
    assert_eq!(body["quotaUsageInBytes"], 0);
    assert!(body["quotaSizeInBytes"].is_null());
    assert!(body["deletedAt"].is_null());
    assert!(!body["avatarColor"].as_str().unwrap().is_empty());

    let (status, body, _) = call(
        &app,
        json_request("GET", "/api/users/me/preferences", None, Some(&cookie)),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["memories"]["enabled"], true);
    assert_eq!(body["download"]["archiveSize"], 4 * 1024 * 1024 * 1024_u64);
    assert_eq!(body["download"]["includeEmbeddedVideos"], false);
    assert_eq!(body["purchase"]["showSupportBadge"], true);
    assert_eq!(body["people"]["updateStrategy"], "everyone");
    let hide_until = body["purchase"]["hideBuyButtonUntil"].as_str().unwrap();
    let parsed = chrono::DateTime::parse_from_rfc3339(hide_until).unwrap();
    let local = parsed.with_timezone(&chrono::Local);
    assert_eq!((local.year(), local.month(), local.day()), (2022, 2, 12));
    assert_eq!((local.hour(), local.minute(), local.second()), (0, 0, 0));

    let (status, body, cleared) = call(
        &app,
        json_request("POST", "/api/auth/logout", None, Some(&cookie)),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["successful"], true);
    assert_eq!(body["redirectUri"], "/auth/login?autoLaunch=0");
    assert!(cleared
        .iter()
        .any(|cookie| cookie.starts_with("immich_access_token=")));

    let (status, body, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", None, Some(&cookie)),
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["message"], "Invalid user token");

    let (status, body, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", None, None),
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["message"], "Authentication required");

    let _ = std::fs::remove_dir_all(&dir);
}

fn sha256(token: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).to_vec()
}
