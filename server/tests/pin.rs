use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;

fn config(dir: &Path) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        host: IpAddr::from([127, 0, 0, 1]),
        port: 0,
        admin_email: Some("admin@example.com".to_string()),
        admin_name: Some("Ada".to_string()),
        admin_password: Some("correct horse".to_string()),
        s3: None,
        web_root: dir.join("web"),
        machine_learning_url: "http://immich-machine-learning:3003".to_string(),
        machine_learning_enabled: false,
        config_file: None,
        log_level: None,
    }
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| panic!("body {}", String::from_utf8_lossy(bytes)))
}

fn request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    api_key: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(api_key) = api_key {
        builder = builder.header("x-api-key", api_key);
    }
    if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        return builder.body(Body::from(body.to_string())).unwrap();
    }
    builder.body(Body::empty()).unwrap()
}

fn jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(8, 6, Rgb([20, 30, 40]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, bytes: &[u8]) -> Request<Body> {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "locked-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("filename", "locked.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").into_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"locked.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
}

async fn sign_in(app: &App, email: &str, password: &str) -> String {
    let response = immich_server::router(app.clone())
        .oneshot(request(
            "POST",
            "/api/auth/login",
            None,
            None,
            Some(&format!(r#"{{"email":"{email}","password":"{password}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn message(bytes: &[u8]) -> String {
    json_body(bytes)["message"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn pin_unlocks_the_locked_folder() {
    let dir = std::env::temp_dir().join(format!("immich-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let cookie = sign_in(&app, "admin@example.com", "correct horse").await;

    let (status, bytes) = call(&app, request("GET", "/api/auth/status", None, None, None)).await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Authentication required");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/auth/status",
            Some("immich_shared_link_token=link"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Forbidden");

    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&cookie), None, None),
    )
    .await;
    let open = json_body(&bytes);
    assert_eq!(status, 200, "{open}");
    assert_eq!(open["pinCode"], false);
    assert_eq!(open["password"], true);
    assert_eq!(open["isElevated"], false);
    assert!(open.get("expiresAt").is_none());
    assert!(open.get("pinExpiresAt").is_none());

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/pin-code",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"12345"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Validation failed");
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/pin-code",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"123456"}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&cookie), None, None),
    )
    .await;
    let armed = json_body(&bytes);
    assert_eq!(status, 200, "{armed}");
    assert_eq!(armed["pinCode"], true);
    assert_eq!(armed["isElevated"], false);
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/pin-code",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"123456"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "User already has a PIN code");

    let (status, bytes) = call(&app, upload(&cookie, &jpeg())).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap().to_string();
    let lock = format!(r#"{{"ids":["{asset_id}"],"visibility":"locked"}}"#);
    let (status, bytes) = call(
        &app,
        request("PUT", "/api/assets", Some(&cookie), None, Some(&lock)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_audit WHERE assetId = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(audits, 0);

    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/timeline/buckets?visibility=locked",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Elevated permission is required");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Not found or no asset.read access");

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/session/unlock",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"000000"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Wrong PIN code");
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/session/unlock",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"123456"}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&cookie), None, None),
    )
    .await;
    let elevated = json_body(&bytes);
    assert_eq!(status, 200, "{elevated}");
    assert_eq!(elevated["isElevated"], true);
    assert!(elevated["pinExpiresAt"].as_str().is_some());

    let soon: String =
        sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%S.000Z', 'now', '+4 minutes')")
            .fetch_one(app.pool())
            .await
            .unwrap();
    sqlx::query("UPDATE session SET pinExpiresAt = ? WHERE userId = (SELECT id FROM user LIMIT 1)")
        .bind(&soon)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        request("GET", "/api/auth/status", Some(&cookie), None, None),
    )
    .await;
    assert_eq!(status, 200);
    let slid: String = sqlx::query_scalar("SELECT pinExpiresAt FROM session LIMIT 1")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert!(slid > soon, "{slid} should be later than {soon}");

    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/timeline/buckets?visibility=locked",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    let buckets = json_body(&bytes);
    assert_eq!(status, 200, "{buckets}");
    assert_eq!(buckets[0]["timeBucket"], "2020-01-01");
    assert_eq!(buckets[0]["count"], 1);
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&visibility=locked",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    let bucket = json_body(&bytes);
    assert_eq!(status, 200, "{bucket}");
    assert_eq!(bucket["id"][0], asset_id);
    assert_eq!(bucket["visibility"][0], "locked");
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            &format!("/api/assets/{asset_id}"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/timeline/buckets?visibility=locked&withPartners=true",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "withPartners is only supported for non-archived, non-trashed, non-favorited, non-locked assets"
    );
    let (status, bytes) = call(
        &app,
        request(
            "GET",
            "/api/timeline/buckets?visibility=locked&userId=00000000-0000-4000-8000-000000000099",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "You may not access another user's locked timeline"
    );

    let other = sign_in(&app, "admin@example.com", "correct horse").await;
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/session/unlock",
            Some(&other),
            None,
            Some(r#"{"pinCode":"123456"}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request("POST", "/api/auth/session/lock", Some(&cookie), None, None),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&cookie), None, None),
    )
    .await;
    let locked_session = json_body(&bytes);
    assert_eq!(status, 200, "{locked_session}");
    assert_eq!(locked_session["isElevated"], false);
    assert!(locked_session.get("pinExpiresAt").is_none());
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&other), None, None),
    )
    .await;
    let other_status = json_body(&bytes);
    assert_eq!(status, 200, "{other_status}");
    assert_eq!(other_status["isElevated"], true);

    let (status, bytes) = call(
        &app,
        request(
            "PUT",
            "/api/auth/pin-code",
            Some(&cookie),
            None,
            Some(r#"{"password":"correct horse","newPinCode":"654321"}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/session/unlock",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"123456"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Wrong PIN code");
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/session/unlock",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"654321"}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&other), None, None),
    )
    .await;
    let still = json_body(&bytes);
    assert_eq!(status, 200, "{still}");
    assert_eq!(still["isElevated"], true);

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/api-keys",
            Some(&cookie),
            None,
            Some(r#"{"name":"clip","permissions":["all"]}"#),
        ),
    )
    .await;
    let key = json_body(&bytes);
    assert_eq!(status, 201, "{key}");
    let secret = key["secret"].as_str().unwrap();
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", None, Some(secret), None),
    )
    .await;
    let keyed = json_body(&bytes);
    assert_eq!(status, 200, "{keyed}");
    assert_eq!(keyed["pinCode"], true);
    assert_eq!(keyed["isElevated"], false);
    assert!(keyed.get("expiresAt").is_none());
    assert!(keyed.get("pinExpiresAt").is_none());
    let (status, bytes) = call(
        &app,
        request("POST", "/api/auth/session/lock", None, Some(secret), None),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "This endpoint can only be used with a session token"
    );

    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/api-keys",
            Some(&cookie),
            None,
            Some(r#"{"name":"narrow","permissions":["session.read"]}"#),
        ),
    )
    .await;
    let narrow = json_body(&bytes);
    assert_eq!(status, 201, "{narrow}");
    let narrow_secret = narrow["secret"].as_str().unwrap();
    let (status, bytes) = call(
        &app,
        request(
            "POST",
            "/api/auth/pin-code",
            None,
            Some(narrow_secret),
            Some(r#"{"pinCode":"111111"}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "Missing required permission: pinCode.create"
    );

    let (status, bytes) = call(
        &app,
        request(
            "DELETE",
            "/api/auth/pin-code",
            Some(&cookie),
            None,
            Some(r#"{"pinCode":"654321"}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&cookie), None, None),
    )
    .await;
    let cleared = json_body(&bytes);
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared["pinCode"], false);
    assert_eq!(cleared["isElevated"], false);
    let (status, bytes) = call(
        &app,
        request("GET", "/api/auth/status", Some(&other), None, None),
    )
    .await;
    let other_cleared = json_body(&bytes);
    assert_eq!(status, 200, "{other_cleared}");
    assert_eq!(other_cleared["isElevated"], false);
    assert!(other_cleared.get("pinExpiresAt").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
