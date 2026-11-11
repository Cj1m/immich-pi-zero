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

fn config(dir: &Path, machine_learning_url: String, enabled: bool) -> Config {
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
        machine_learning_enabled: enabled,
        config_file: None,
        log_level: None,
    }
}

async fn ml_server() -> String {
    let vector: Vec<f64> = std::iter::once(1.0)
        .chain(std::iter::repeat_n(0.0, 511))
        .collect();
    let clip = serde_json::to_string(&vector).unwrap();
    let embedding = vector.clone();
    let router = Router::new()
        .route("/ping", get(|| async { "pong" }))
        .route(
            "/predict",
            post(move |request: Request| {
                let clip = clip.clone();
                let embedding = embedding.clone();
                async move {
                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                    let text = String::from_utf8_lossy(&bytes);
                    let body = if text.contains("facial-recognition") {
                        json!({
                            "imageWidth": 8,
                            "imageHeight": 8,
                            "facial-recognition": [{
                                "boundingBox": { "x1": 1, "y1": 1, "x2": 5, "y2": 5 },
                                "embedding": embedding,
                                "score": 0.99
                            }]
                        })
                    } else if text.contains("\"ocr\"") {
                        json!({
                            "ocr": {
                                "text": ["hello"],
                                "box": [1.0, 2.0, 3.0, 2.0, 3.0, 4.0, 1.0, 4.0],
                                "boxScore": [0.9],
                                "textScore": [0.95]
                            }
                        })
                    } else {
                        json!({ "clip": clip, "imageHeight": 8, "imageWidth": 8 })
                    };
                    Json(body)
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
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn jpeg(color: [u8; 3]) -> Vec<u8> {
    let image = RgbImage::from_pixel(16, 10, Rgb(color));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn multipart(bytes: &[u8], device: &str, filename: &str, created: &str) -> (String, Vec<u8>) {
    let boundary = "immichboundary";
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", device),
        ("deviceId", "phone"),
        ("fileCreatedAt", created),
        ("fileModifiedAt", created),
        ("filename", filename),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"{filename}\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
        .into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
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

fn json_request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .unwrap()
}

async fn upload(
    app: &App,
    cookie: &str,
    device: &str,
    filename: &str,
    created: &str,
    color: [u8; 3],
) -> String {
    let (content_type, body) = multipart(&jpeg(color), device, filename, created);
    let (status, bytes) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/assets")
            .header("content-type", content_type)
            .header("cookie", cookie)
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    let created: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 201, "{created}");
    created["id"].as_str().unwrap().to_string()
}

async fn faces_of(app: &App, cookie: &str, asset_id: &str) -> Value {
    let (status, bytes) = call(
        app,
        json_request(
            "GET",
            &format!("/api/faces?id={asset_id}"),
            Some(cookie),
            None,
        ),
    )
    .await;
    let faces: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "{faces}");
    faces
}

#[tokio::test]
async fn faces_group_one_person_and_ocr_is_readable() {
    let dir = std::env::temp_dir().join(format!("immich-vision-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir, ml_server().await, true))
        .await
        .expect("prepare");
    let cookie = session(&app).await;
    let when = "2020-06-01T12:00:00.000Z";
    let first = upload(&app, &cookie, "device-a", "a.jpg", when, [12, 40, 90]).await;
    let second = upload(&app, &cookie, "device-b", "b.jpg", when, [90, 12, 40]).await;
    let third = upload(&app, &cookie, "device-c", "c.jpg", when, [40, 90, 12]).await;

    app.detect_missing_faces().await;
    app.recognize_unassigned_faces().await;
    app.read_missing_ocr().await;

    let first_faces = faces_of(&app, &cookie, &first).await;
    assert_eq!(first_faces.as_array().unwrap().len(), 1, "{first_faces}");
    assert_eq!(first_faces[0]["sourceType"], "machine-learning");
    assert_eq!(first_faces[0]["imageWidth"], 8);
    assert_eq!(first_faces[0]["imageHeight"], 8);
    assert_eq!(first_faces[0]["boundingBoxX1"], 1);
    assert_eq!(first_faces[0]["boundingBoxY1"], 1);
    assert_eq!(first_faces[0]["boundingBoxX2"], 5);
    assert_eq!(first_faces[0]["boundingBoxY2"], 5);
    let person_id = first_faces[0]["person"]["id"].as_str().unwrap().to_string();
    let face_id = first_faces[0]["id"].as_str().unwrap().to_string();
    for asset_id in [&second, &third] {
        let faces = faces_of(&app, &cookie, asset_id).await;
        assert_eq!(faces.as_array().unwrap().len(), 1, "{faces}");
        assert_eq!(faces[0]["person"]["id"], person_id);
    }

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/people", Some(&cookie), None),
    )
    .await;
    let people: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "{people}");
    assert_eq!(people["total"], 1);
    assert_eq!(people["people"][0]["id"], person_id);
    assert_eq!(people["people"][0]["name"], "");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{first}/ocr"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let regions: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "{regions}");
    assert_eq!(regions.as_array().unwrap().len(), 1);
    assert_eq!(regions[0]["text"], "hello");
    assert_eq!(regions[0]["x1"], 1.0);
    assert_eq!(regions[0]["y4"], 4.0);

    sqlx::query("UPDATE asset_job_status SET facesRecognizedAt = NULL WHERE assetId = ?")
        .bind(&first)
        .execute(app.pool())
        .await
        .unwrap();
    app.detect_missing_faces().await;
    let again = faces_of(&app, &cookie, &first).await;
    assert_eq!(again.as_array().unwrap().len(), 1, "{again}");
    assert_eq!(again[0]["id"], face_id);
    assert_eq!(again[0]["person"]["id"], person_id);
    assert_eq!(again[0]["sourceType"], "machine-learning");

    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/people/{person_id}"),
            Some(&cookie),
            Some(r#"{"birthDate":"2000-01-01"}"#),
        ),
    )
    .await;
    let updated: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["birthDate"], "2000-01-01");

    let earlier = upload(
        &app,
        &cookie,
        "device-f",
        "f.jpg",
        "1990-01-01T00:00:00.000Z",
        [1, 2, 3],
    )
    .await;
    app.detect_missing_faces().await;
    app.recognize_unassigned_faces().await;
    let early_faces = faces_of(&app, &cookie, &earlier).await;
    assert_eq!(early_faces.as_array().unwrap().len(), 1, "{early_faces}");
    assert!(early_faces[0]["person"].is_null(), "{early_faces}");
    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/people", Some(&cookie), None),
    )
    .await;
    let people: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "{people}");
    assert_eq!(people["total"], 1);

    let manual_asset = upload(&app, &cookie, "device-d", "d.jpg", when, [200, 100, 50]).await;
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/people", Some(&cookie), Some("{}")),
    )
    .await;
    let manual_person: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 201, "{manual_person}");
    let manual_person_id = manual_person["id"].as_str().unwrap();
    let face_body = format!(
        r#"{{"assetId":"{manual_asset}","personId":"{manual_person_id}","imageWidth":8,"imageHeight":8,"x":1,"y":1,"width":4,"height":4}}"#
    );
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/faces", Some(&cookie), Some(&face_body)),
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    app.detect_missing_faces().await;
    let manual_faces = faces_of(&app, &cookie, &manual_asset).await;
    assert_eq!(manual_faces.as_array().unwrap().len(), 1, "{manual_faces}");
    assert_eq!(manual_faces[0]["sourceType"], "manual");
    assert_eq!(manual_faces[0]["person"]["id"], manual_person_id);
    let manual_face_id = manual_faces[0]["id"].as_str().unwrap();
    let embedding_bytes: i64 =
        sqlx::query_scalar("SELECT length(embedding) FROM face_search WHERE faceId = ?")
            .bind(manual_face_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(embedding_bytes, 512 * 4);

    app.read_missing_ocr().await;
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{manual_asset}/ocr"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let regions: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 200, "{regions}");
    assert_eq!(regions[0]["text"], "hello");
}

#[tokio::test]
async fn disabled_face_and_ocr_jobs_leave_assets_alone() {
    let dir = std::env::temp_dir().join(format!("immich-vision-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir, "http://127.0.0.1:9".to_string(), false))
        .await
        .expect("prepare");
    let cookie = session(&app).await;
    let asset_id = upload(
        &app,
        &cookie,
        "device-off",
        "off.jpg",
        "2020-06-01T12:00:00.000Z",
        [8, 8, 8],
    )
    .await;
    app.detect_missing_faces().await;
    app.recognize_unassigned_faces().await;
    app.read_missing_ocr().await;
    let faces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_face WHERE assetId = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    let regions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset_ocr WHERE assetId = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    let stamped: Option<String> =
        sqlx::query_scalar("SELECT facesRecognizedAt FROM asset_job_status WHERE assetId = ?")
            .bind(&asset_id)
            .fetch_optional(app.pool())
            .await
            .unwrap();
    let read: Option<String> =
        sqlx::query_scalar("SELECT ocrAt FROM asset_job_status WHERE assetId = ?")
            .bind(&asset_id)
            .fetch_optional(app.pool())
            .await
            .unwrap();
    assert_eq!(faces, 0);
    assert_eq!(regions, 0);
    // SQLite NULL text comes back as an empty string through this scalar.
    assert!(stamped.as_deref().unwrap_or("").is_empty(), "{stamped:?}");
    assert!(read.as_deref().unwrap_or("").is_empty(), "{read:?}");
}
