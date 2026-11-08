use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::{json, Value};
use tower::ServiceExt;

fn config(dir: &Path, machine_learning_url: String) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        host: IpAddr::from([127, 0, 0, 1]),
        port: 0,
        admin_email: Some("admin@example.com".to_string()),
        admin_name: Some("Ada".to_string()),
        admin_password: Some("correct horse".to_string()),
        s3: None,
        web_root: dir.join("web"),
        machine_learning_url,
        machine_learning_enabled: true,
        config_file: None,
        log_level: None,
    }
}

async fn ml_server() -> String {
    let vector: Vec<f64> = std::iter::once(1.0)
        .chain(std::iter::repeat_n(0.0, 511))
        .collect();
    let clip = serde_json::to_string(&vector).unwrap();
    let router = Router::new().route("/ping", get(|| async { "pong" })).route(
        "/predict",
        post(move || {
            let clip = clip.clone();
            async move {
                Json(json!({
                    "clip": clip,
                    "imageHeight": 8,
                    "imageWidth": 8,
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    for _ in 0..50 {
        if client
            .get(format!("{url}/ping"))
            .send()
            .await
            .ok()
            .is_some_and(|response| response.status().is_success())
        {
            return url;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("machine learning mock did not answer ping");
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(16, 10, Rgb([12, 40, 90]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn multipart(bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-05-04T03:02:01.000Z"),
        ("fileModifiedAt", "2020-05-04T03:02:01.000Z"),
        ("filename", "cat.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        b"--immichboundary\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"cat.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n",
    );
    body.extend_from_slice(bytes);
    body.extend(b"\r\n--immichboundary--\r\n");
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn session(app: &App) -> String {
    let response = immich_server::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correct horse"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .expect("cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn json_request(method: &str, path: &str, cookie: Option<&str>, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder.body(Body::from(body.unwrap_or("").to_string())).unwrap()
}

#[tokio::test]
async fn smart_search_uses_the_stored_clip_vector() {
    let dir = std::env::temp_dir().join(format!("immich-smart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ml_url = ml_server().await;
    let app = App::prepare(&config(&dir, ml_url)).await.expect("prepare");
    let cookie = session(&app).await;

    let photo = jpeg();
    let (content_type, upload_body) = multipart(&photo);
    let (status, body) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/assets")
            .header("content-type", content_type)
            .header("cookie", &cookie)
            .body(Body::from(upload_body))
            .unwrap(),
    )
    .await;
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let bytes: i64 = sqlx::query_scalar("SELECT length(embedding) FROM asset_clip WHERE assetId = ?")
        .bind(&id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(bytes, 512 * 4);

    let (status, body) = call(&app, json_request("POST", "/api/search/smart", None, Some("{}"))).await;
    let missing: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["message"], "Authentication required");

    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/smart", Some(&cookie), Some("{}")),
    )
    .await;
    let empty: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{empty}");
    assert_eq!(empty["message"], "Either `query` or `queryAssetId` must be set");

    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/smart", Some(&cookie), Some(r#"{"query":"cat"}"#)),
    )
    .await;
    let found: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{found}");
    assert_eq!(found["assets"]["count"], 1);
    assert_eq!(found["assets"]["total"], 1);
    assert_eq!(found["assets"]["items"][0]["id"], id);
    assert!(found["assets"]["nextPage"].is_null());
    assert!(found["assets"]["nextCursor"].is_null());
    assert_eq!(found["albums"]["count"], 0);

    let by_asset = format!(r#"{{"queryAssetId":"{id}"}}"#);
    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/smart", Some(&cookie), Some(&by_asset)),
    )
    .await;
    let same: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{same}");
    assert_eq!(same["assets"]["items"][0]["id"], id);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/search/smart",
            Some(&cookie),
            Some(r#"{"queryAssetId":"00000000-0000-4000-8000-000000000000"}"#),
        ),
    )
    .await;
    let unknown: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{unknown}");
    assert_eq!(unknown["message"], "Not found or no asset.read access");

    let (status, config_body) = call(
        &app,
        json_request("GET", "/api/system-config", Some(&cookie), None),
    )
    .await;
    let mut document: Value = serde_json::from_slice(&config_body).unwrap();
    assert_eq!(status, 200, "{document}");
    document["machineLearning"]["clip"]["enabled"] = json!(false);
    let (status, saved_body) = call(
        &app,
        json_request("PUT", "/api/system-config", Some(&cookie), Some(&document.to_string())),
    )
    .await;
    let saved: Value = serde_json::from_slice(&saved_body).unwrap();
    assert_eq!(status, 200, "{saved}");
    let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_clip")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(still, 1);
    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/smart", Some(&cookie), Some(r#"{"query":"cat"}"#)),
    )
    .await;
    let disabled: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{disabled}");
    assert_eq!(disabled["message"], "Smart search is not enabled");

    document["machineLearning"]["clip"]["enabled"] = json!(true);
    let (status, saved_body) = call(
        &app,
        json_request("PUT", "/api/system-config", Some(&cookie), Some(&document.to_string())),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&saved_body));

    sqlx::query("DELETE FROM asset_clip").execute(app.pool()).await.unwrap();
    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/smart", Some(&cookie), Some(&by_asset)),
    )
    .await;
    let missing_vector: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{missing_vector}");
    assert_eq!(missing_vector["message"], format!("Asset {id} has no embedding"));

    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/smart", Some(&cookie), Some(r#"{"query":"cat"}"#)),
    )
    .await;
    let none: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{none}");
    assert_eq!(none["assets"]["count"], 0);

    let _ = std::fs::remove_dir_all(&dir);
}
