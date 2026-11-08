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
    let router = Router::new()
        .route("/ping", get(|| async { "pong" }))
        .route(
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
    if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        return builder.body(Body::from(body.to_string())).unwrap();
    }
    builder.body(Body::empty()).unwrap()
}

fn jpeg(width: u32, height: u32, red: u8) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, _y| Rgb([red, (x % 251) as u8, 90]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, name: &str, created: &str, favorite: bool, bytes: &[u8]) -> Request<Body> {
    let boundary = "immichboundary";
    let favorite = if favorite { "true" } else { "false" };
    let mut body = Vec::new();
    for (field, value) in [
        ("deviceAssetId", name),
        ("deviceId", "phone"),
        ("fileCreatedAt", created),
        ("fileModifiedAt", created),
        ("isFavorite", favorite),
        ("filename", name),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"{name}\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
        .into_bytes(),
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

async fn sign_in(app: &App, email: &str, password: &str) -> (String, String) {
    let response = immich_server::router(app.clone())
        .oneshot(json_request(
            "POST",
            "/api/auth/login",
            None,
            Some(&format!(r#"{{"email":"{email}","password":"{password}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("immich_access_token="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let login = json_body(&bytes);
    (cookie, login["userId"].as_str().unwrap().to_string())
}

async fn add_user(app: &App, cookie: &str, email: &str, name: &str) -> String {
    let body = format!(r#"{{"email":"{email}","password":"partner-secret","name":"{name}"}}"#);
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/admin/users", Some(cookie), Some(&body)),
    )
    .await;
    let user = json_body(&bytes);
    assert_eq!(status, 201, "{user}");
    user["id"].as_str().unwrap().to_string()
}

async fn upload_asset(
    app: &App,
    cookie: &str,
    name: &str,
    created: &str,
    favorite: bool,
    width: u32,
    red: u8,
) -> String {
    let (status, bytes) = call(
        app,
        upload(
            cookie,
            name,
            created,
            favorite,
            &jpeg(width, width / 2, red),
        ),
    )
    .await;
    let asset = json_body(&bytes);
    assert_eq!(status, 201, "{asset}");
    asset["id"].as_str().unwrap().to_string()
}

async fn duplicate_of(app: &App, id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT duplicateId FROM asset WHERE id = ?")
        .bind(id)
        .fetch_one(app.pool())
        .await
        .unwrap()
}

async fn asset_of(app: &App, cookie: &str, id: &str) -> Value {
    let (status, bytes) = call(
        app,
        json_request("GET", &format!("/api/assets/{id}"), Some(cookie), None),
    )
    .await;
    let asset = json_body(&bytes);
    assert_eq!(status, 200, "{asset}");
    asset
}

#[tokio::test]
async fn duplicate_groups_follow_the_clip_vector() {
    let dir = std::env::temp_dir().join(format!("immich-duplicates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ml_url = ml_server().await;
    let app = App::prepare(&config(&dir, ml_url)).await.expect("prepare");
    let (cookie, owner) = sign_in(&app, "admin@example.com", "correct horse").await;
    let bob = add_user(&app, &cookie, "bob@example.com", "Bob").await;
    let (bob_cookie, _) = sign_in(&app, "bob@example.com", "partner-secret").await;

    let (status, bytes) = call(&app, json_request("GET", "/api/duplicates", None, None)).await;
    let unsigned = json_body(&bytes);
    assert_eq!(status, 401, "{unsigned}");
    assert_eq!(unsigned["message"], "Authentication required");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/duplicates",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    let shared = json_body(&bytes);
    assert_eq!(status, 403, "{shared}");
    assert_eq!(shared["message"], "Forbidden");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/duplicates?key=nope", None, None),
    )
    .await;
    let bad_key = json_body(&bytes);
    assert_eq!(status, 401, "{bad_key}");
    assert_eq!(bad_key["message"], "Invalid share key");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/server/features", None, None),
    )
    .await;
    let features = json_body(&bytes);
    assert_eq!(status, 200, "{features}");
    assert_eq!(features["duplicateDetection"], true);

    let large = upload_asset(
        &app,
        &cookie,
        "a.jpg",
        "2019-01-01T00:00:00.000Z",
        true,
        400,
        20,
    )
    .await;
    let small = upload_asset(
        &app,
        &cookie,
        "b.jpg",
        "2021-06-01T00:00:00.000Z",
        false,
        16,
        80,
    )
    .await;

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/duplicates", Some(&cookie), None),
    )
    .await;
    let listed = json_body(&bytes);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let group = listed[0]["duplicateId"].as_str().unwrap().to_string();
    assert_eq!(listed[0]["assets"].as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["assets"][0]["id"], large);
    assert_eq!(listed[0]["assets"][1]["id"], small);
    assert_eq!(listed[0]["assets"][0]["duplicateId"], group);
    assert_eq!(listed[0]["assets"][1]["duplicateId"], group);
    let large_size = listed[0]["assets"][0]["exifInfo"]["fileSizeInByte"]
        .as_i64()
        .unwrap();
    let small_size = listed[0]["assets"][1]["exifInfo"]["fileSizeInByte"]
        .as_i64()
        .unwrap();
    assert!(large_size > small_size, "{large_size} vs {small_size}");
    assert_eq!(listed[0]["suggestedKeepAssetIds"][0], large);

    let both = format!(
        r#"{{"groups":[{{"duplicateId":"{group}","keepAssetIds":["{large}","{small}"],"trashAssetIds":["{large}"]}}]}}"#
    );
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/duplicates/resolve",
            Some(&cookie),
            Some(&both),
        ),
    )
    .await;
    let both_body = json_body(&bytes);
    assert_eq!(status, 200, "{both_body}");
    assert_eq!(both_body[0]["success"], false);
    assert_eq!(both_body[0]["error"], "validation");
    assert_eq!(
        both_body[0]["errorMessage"],
        "An asset cannot be in both keepAssetIds and trashAssetIds"
    );

    let neither = format!(
        r#"{{"groups":[{{"duplicateId":"{group}","keepAssetIds":["{large}"],"trashAssetIds":[]}}]}}"#
    );
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/duplicates/resolve",
            Some(&cookie),
            Some(&neither),
        ),
    )
    .await;
    let neither_body = json_body(&bytes);
    assert_eq!(status, 200, "{neither_body}");
    assert_eq!(neither_body[0]["error"], "validation");
    assert_eq!(
        neither_body[0]["errorMessage"],
        "Every asset must be in either keepAssetIds or trashAssetIds"
    );

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/duplicates/resolve",
            Some(&cookie),
            Some(r#"{"groups":[]}"#),
        ),
    )
    .await;
    let empty_groups = json_body(&bytes);
    assert_eq!(status, 400, "{empty_groups}");
    assert_eq!(empty_groups["message"], "Validation failed");
    assert_eq!(
        duplicate_of(&app, &large).await.as_deref(),
        Some(group.as_str())
    );
    assert_eq!(
        duplicate_of(&app, &small).await.as_deref(),
        Some(group.as_str())
    );

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/duplicates/{group}"),
            Some(&bob_cookie),
            None,
        ),
    )
    .await;
    let foreign = json_body(&bytes);
    assert_eq!(status, 400, "{foreign}");
    assert_eq!(
        foreign["message"],
        "Not found or no duplicate.delete access"
    );
    assert_eq!(
        duplicate_of(&app, &large).await.as_deref(),
        Some(group.as_str())
    );

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/duplicates/{group}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    assert!(bytes.is_empty());
    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/duplicates", Some(&cookie), None),
    )
    .await;
    let cleared = json_body(&bytes);
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared.as_array().unwrap().len(), 0);
    assert_eq!(duplicate_of(&app, &large).await, None);

    let later = upload_asset(
        &app,
        &cookie,
        "c.jpg",
        "2022-01-01T00:00:00.000Z",
        false,
        48,
        140,
    )
    .await;
    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/duplicates", Some(&cookie), None),
    )
    .await;
    let regrouped = json_body(&bytes);
    assert_eq!(status, 200, "{regrouped}");
    assert_eq!(regrouped.as_array().unwrap().len(), 1);
    let group = regrouped[0]["duplicateId"].as_str().unwrap().to_string();
    assert_eq!(regrouped[0]["assets"].as_array().unwrap().len(), 3);
    assert_eq!(regrouped[0]["suggestedKeepAssetIds"][0], large);
    let stranger = "00000000-0000-4000-8000-000000000099";
    let mixed = format!(
        r#"{{"groups":[{{"duplicateId":"{group}","keepAssetIds":["{small}"],"trashAssetIds":["{large}","{later}"]}},{{"duplicateId":"{stranger}","keepAssetIds":[],"trashAssetIds":[]}}]}}"#
    );
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/duplicates/resolve",
            Some(&cookie),
            Some(&mixed),
        ),
    )
    .await;
    let denied = json_body(&bytes);
    assert_eq!(status, 400, "{denied}");
    assert_eq!(denied["message"], "Not found or no duplicate.delete access");
    assert_eq!(
        duplicate_of(&app, &large).await.as_deref(),
        Some(group.as_str())
    );
    assert_eq!(
        duplicate_of(&app, &later).await.as_deref(),
        Some(group.as_str())
    );

    for (id, description) in [(&large, "alpha"), (&small, "beta"), (&later, "")] {
        sqlx::query("UPDATE asset_exif SET description = ? WHERE assetId = ?")
            .bind(description)
            .bind(id)
            .execute(app.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE asset_exif SET latitude = 10.5, longitude = -20.25 WHERE assetId = ?")
            .bind(id)
            .execute(app.pool())
            .await
            .unwrap();
    }
    sqlx::query("UPDATE asset_exif SET rating = 5 WHERE assetId = ?")
        .bind(&small)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE asset SET visibility = 'archive' WHERE id = ?")
        .bind(&small)
        .execute(app.pool())
        .await
        .unwrap();
    let album_body = format!(r#"{{"albumName":"Trip","assetIds":["{large}"]}}"#);
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/albums", Some(&cookie), Some(&album_body)),
    )
    .await;
    let album = json_body(&bytes);
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap();

    let resolve = format!(
        r#"{{"groups":[{{"duplicateId":"{group}","keepAssetIds":["{small}"],"trashAssetIds":["{large}","{later}"]}}]}}"#
    );
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/duplicates/resolve",
            Some(&cookie),
            Some(&resolve),
        ),
    )
    .await;
    let resolved = json_body(&bytes);
    assert_eq!(status, 200, "{resolved}");
    assert_eq!(resolved[0]["id"], group);
    assert_eq!(resolved[0]["success"], true);
    assert!(resolved[0].get("error").is_none());

    let kept = asset_of(&app, &cookie, &small).await;
    assert_eq!(kept["isFavorite"], true);
    assert_eq!(kept["visibility"], "archive");
    assert_eq!(kept["isTrashed"], false);
    assert!(kept["duplicateId"].is_null());
    assert_eq!(kept["exifInfo"]["description"], "alpha\nbeta");
    assert_eq!(kept["exifInfo"]["rating"], 5);
    assert_eq!(kept["exifInfo"]["latitude"], 10.5);
    assert_eq!(kept["exifInfo"]["longitude"], -20.25);
    let in_album: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM album_asset WHERE albumId = ? AND assetId = ?")
            .bind(album_id)
            .bind(&small)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(in_album, 1);
    let trashed_large = asset_of(&app, &cookie, &large).await;
    let trashed_later = asset_of(&app, &cookie, &later).await;
    assert_eq!(trashed_large["isTrashed"], true);
    assert_eq!(trashed_later["isTrashed"], true);
    assert!(trashed_large["duplicateId"].is_null());
    assert!(trashed_later["duplicateId"].is_null());
    let sidecar = std::fs::read_to_string(
        dir.join("objects")
            .join(&owner)
            .join(&small)
            .join("original.xmp"),
    )
    .unwrap();
    assert!(sidecar.contains("alpha"), "{sidecar}");
    assert!(sidecar.contains("beta"), "{sidecar}");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/duplicates", Some(&cookie), None),
    )
    .await;
    let empty = json_body(&bytes);
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty.as_array().unwrap().len(), 0);

    let stack_a = "00000000-0000-4000-8000-000000000011";
    let stack_b = "00000000-0000-4000-8000-000000000012";
    let stack_group = "00000000-0000-4000-8000-000000000013";
    for id in [stack_a, stack_b] {
        sqlx::query(
            "INSERT INTO asset (
                id, ownerId, type, originalPath, originalFileName, originalMimeType,
                fileCreatedAt, fileModifiedAt, localDateTime, checksum, updateId, createdAt, updatedAt
             ) VALUES (
                ?, ?, 'IMAGE', 'object', 'stack.jpg', 'image/jpeg',
                '2020-01-02T03:04:05.000Z', '2020-01-02T03:04:05.000Z', '2020-01-02T03:04:05.000Z',
                ?, 'stack-update', '2020-01-02T03:04:05.000Z', '2020-01-02T03:04:05.000Z'
             )",
        )
        .bind(id)
        .bind(&owner)
        .bind(id.as_bytes())
        .execute(app.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE asset SET duplicateId = ? WHERE id = ?")
            .bind(stack_group)
            .bind(id)
            .execute(app.pool())
            .await
            .unwrap();
    }
    let mixed_put = format!(r#"{{"ids":["{stack_a}","{bob}"],"duplicateId":null}}"#);
    let (status, bytes) = call(
        &app,
        json_request("PUT", "/api/assets", Some(&cookie), Some(&mixed_put)),
    )
    .await;
    let mixed_body = json_body(&bytes);
    assert_eq!(status, 400, "{mixed_body}");
    assert_eq!(mixed_body["message"], "Not found or no asset.update access");
    assert_eq!(
        duplicate_of(&app, stack_a).await.as_deref(),
        Some(stack_group)
    );

    let stack_body = format!(r#"{{"assetIds":["{stack_a}","{stack_b}"]}}"#);
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/stacks", Some(&cookie), Some(&stack_body)),
    )
    .await;
    let stack = json_body(&bytes);
    assert_eq!(status, 201, "{stack}");
    assert_eq!(stack["primaryAssetId"], stack_a);
    let clear = format!(r#"{{"ids":["{stack_a}","{stack_b}"],"duplicateId":null}}"#);
    let (status, bytes) = call(
        &app,
        json_request("PUT", "/api/assets", Some(&cookie), Some(&clear)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    assert!(bytes.is_empty());
    assert_eq!(duplicate_of(&app, stack_a).await, None);
    assert_eq!(duplicate_of(&app, stack_b).await, None);
    let stacked = asset_of(&app, &cookie, stack_a).await;
    assert_eq!(stacked["stack"]["primaryAssetId"], stack_a);
    assert!(stacked["duplicateId"].is_null());

    let _ = std::fs::remove_dir_all(&dir);
}
