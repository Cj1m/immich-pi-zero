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

fn jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(8, 6, Rgb([12, 40, 90]));
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
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("filename", "cat.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").into_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"cat.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").into_bytes(),
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
    (
        cookie,
        json_body(&bytes)["userId"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn tags_and_exif_edits_write_a_sidecar() {
    let dir = std::env::temp_dir().join(format!("immich-tags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (cookie, owner) = sign_in(&app, "admin@example.com", "correct horse").await;
    let bob_body = r#"{"email":"bob@example.com","password":"partner-secret","name":"Bob"}"#;
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/admin/users", Some(&cookie), Some(bob_body)),
    )
    .await;
    let bob = json_body(&bytes);
    assert_eq!(status, 201, "{bob}");
    let (bob_cookie, _) = sign_in(&app, "bob@example.com", "partner-secret").await;

    let (status, bytes) = call(&app, json_request("GET", "/api/tags", None, None)).await;
    let unsigned = json_body(&bytes);
    assert_eq!(status, 401, "{unsigned}");
    assert_eq!(unsigned["message"], "Authentication required");
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/tags",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    let shared = json_body(&bytes);
    assert_eq!(status, 403, "{shared}");
    assert_eq!(shared["message"], "Forbidden");

    let (status, bytes) = call(&app, json_request("GET", "/api/tags", Some(&cookie), None)).await;
    let empty = json_body(&bytes);
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty.as_array().unwrap().len(), 0);

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/tags",
            Some(&cookie),
            Some(r#"{"name":"Trip/Beach"}"#),
        ),
    )
    .await;
    let slashed = json_body(&bytes);
    assert_eq!(status, 400, "{slashed}");
    assert_eq!(slashed["message"], "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/tags",
            Some(&cookie),
            Some(r##"{"name":"Trip","color":"#336699"}"##),
        ),
    )
    .await;
    let trip = json_body(&bytes);
    assert_eq!(status, 201, "{trip}");
    assert_eq!(trip["name"], "Trip");
    assert_eq!(trip["value"], "Trip");
    assert_eq!(trip["color"], "#336699");
    assert!(trip.get("parentId").is_none());
    let trip_id = trip["id"].as_str().unwrap();

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/tags",
            Some(&cookie),
            Some(r#"{"name":"Trip"}"#),
        ),
    )
    .await;
    let duplicate = json_body(&bytes);
    assert_eq!(status, 400, "{duplicate}");
    assert_eq!(duplicate["message"], "A tag with that name already exists");

    let child_body = format!(r#"{{"name":"Beach","parentId":"{trip_id}"}}"#);
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/tags", Some(&cookie), Some(&child_body)),
    )
    .await;
    let beach = json_body(&bytes);
    assert_eq!(status, 201, "{beach}");
    assert_eq!(beach["name"], "Beach");
    assert_eq!(beach["value"], "Trip/Beach");
    assert_eq!(beach["parentId"], trip_id);
    let beach_id = beach["id"].as_str().unwrap().to_string();

    let missing_parent = r#"{"name":"Nope","parentId":"00000000-0000-4000-8000-000000000099"}"#;
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/tags", Some(&cookie), Some(missing_parent)),
    )
    .await;
    let missing = json_body(&bytes);
    assert_eq!(status, 400, "{missing}");
    assert_eq!(missing["message"], "Not found or no tag.read access");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/tags/{beach_id}"),
            Some(&bob_cookie),
            None,
        ),
    )
    .await;
    let foreign = json_body(&bytes);
    assert_eq!(status, 400, "{foreign}");
    assert_eq!(foreign["message"], "Not found or no tag.read access");

    let (status, bytes) = call(&app, upload(&cookie, &jpeg())).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap().to_string();

    let described = format!(r#"{{"ids":["{asset_id}"],"description":"hello"}}"#);
    let (status, bytes) = call(
        &app,
        json_request("PUT", "/api/assets", Some(&cookie), Some(&described)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let dated =
        format!(r#"{{"ids":["{asset_id}"],"dateTimeOriginal":"2024-01-01T00:00:00.000Z"}}"#);
    let (status, bytes) = call(
        &app,
        json_request("PATCH", "/api/assets", Some(&cookie), Some(&dated)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{asset_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let asset = json_body(&bytes);
    assert_eq!(status, 200, "{asset}");
    assert_eq!(asset["exifInfo"]["description"], "hello");
    assert_eq!(
        asset["exifInfo"]["dateTimeOriginal"],
        "2024-01-01T00:00:00.000Z"
    );
    assert_eq!(asset["exifInfo"]["timeZone"], "UTC");
    let shifted = format!(r#"{{"ids":["{asset_id}"],"dateTimeRelative":60}}"#);
    let (status, bytes) = call(
        &app,
        json_request("PUT", "/api/assets", Some(&cookie), Some(&shifted)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{asset_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let shifted_asset = json_body(&bytes);
    assert_eq!(status, 200, "{shifted_asset}");
    assert_eq!(
        shifted_asset["exifInfo"]["dateTimeOriginal"],
        "2024-01-01T01:00:00.000Z"
    );

    let tag_assets = format!(r#"{{"ids":["{asset_id}"]}}"#);
    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/tags/{beach_id}/assets"),
            Some(&cookie),
            Some(&tag_assets),
        ),
    )
    .await;
    let tagged = json_body(&bytes);
    assert_eq!(status, 200, "{tagged}");
    assert_eq!(tagged[0]["success"], true);
    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/tags/{beach_id}/assets"),
            Some(&cookie),
            Some(&tag_assets),
        ),
    )
    .await;
    let again = json_body(&bytes);
    assert_eq!(status, 200, "{again}");
    assert_eq!(again[0]["error"], "duplicate");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{asset_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let with_tag = json_body(&bytes);
    assert_eq!(status, 200, "{with_tag}");
    assert_eq!(with_tag["tags"][0]["value"], "Trip/Beach");
    let sidecar = std::fs::read_to_string(
        dir.join("objects")
            .join(&owner)
            .join(&asset_id)
            .join("original.xmp"),
    )
    .unwrap();
    assert!(sidecar.contains("hello"), "{sidecar}");
    assert!(sidecar.contains("Trip/Beach"), "{sidecar}");
    assert!(sidecar.contains("2024-01-01T01:00:00.000Z"), "{sidecar}");

    let renamed = r#"{"name":"Holiday"}"#.to_string();
    let (status, bytes) = call(
        &app,
        json_request(
            "PATCH",
            &format!("/api/tags/{trip_id}"),
            Some(&cookie),
            Some(&renamed),
        ),
    )
    .await;
    let holiday = json_body(&bytes);
    assert_eq!(status, 200, "{holiday}");
    assert_eq!(holiday["value"], "Holiday");
    let (status, bytes) = call(
        &app,
        json_request("GET", &format!("/api/tags/{beach_id}"), Some(&cookie), None),
    )
    .await;
    let renamed_child = json_body(&bytes);
    assert_eq!(status, 200, "{renamed_child}");
    assert_eq!(renamed_child["value"], "Holiday/Beach");
    let tags_json: String = sqlx::query_scalar("SELECT tags FROM asset_exif WHERE assetId = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(tags_json, r#"["Holiday/Beach"]"#);

    let album_body = format!(r#"{{"albumName":"Days","assetIds":["{asset_id}"]}}"#);
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/albums", Some(&cookie), Some(&album_body)),
    )
    .await;
    let album = json_body(&bytes);
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap();
    let lock = format!(r#"{{"ids":["{asset_id}"],"visibility":"locked"}}"#);
    let (status, bytes) = call(
        &app,
        json_request("PUT", "/api/assets", Some(&cookie), Some(&lock)),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM album_asset WHERE albumId = ? AND assetId = ?")
            .bind(album_id)
            .bind(&asset_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(left, 0);

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/tags/{trip_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    let (status, bytes) = call(&app, json_request("GET", "/api/tags", Some(&cookie), None)).await;
    let after = json_body(&bytes);
    assert_eq!(status, 200, "{after}");
    assert_eq!(after.as_array().unwrap().len(), 0);
    let cleared: String = sqlx::query_scalar("SELECT tags FROM asset_exif WHERE assetId = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(cleared, "[]");

    let _ = std::fs::remove_dir_all(&dir);
}
