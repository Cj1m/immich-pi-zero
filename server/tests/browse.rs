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
        machine_learning_enabled: true,
        config_file: None,
        log_level: None,
    }
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>) {
    let response = immich_server::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn jpeg(red: u8) -> Vec<u8> {
    let image = RgbImage::from_pixel(16, 10, Rgb([red, 40, 90]));
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
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"cat.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n")
            .into_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    Request::builder()
        .method("POST")
        .uri("/api/assets")
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .header("cookie", cookie)
        .body(Body::from(body))
        .unwrap()
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
async fn map_explore_and_folders_read_the_library() {
    let dir = std::env::temp_dir().join(format!("immich-browse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let response = immich_server::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"admin@example.com","password":"correct horse"}"#))
                .unwrap(),
        )
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

    let mut ids = Vec::new();
    for red in [0_u8, 64, 128, 192, 255] {
        let (status, body) = call(&app, upload(&cookie, &jpeg(red))).await;
        let asset: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, 201, "{asset}");
        let id = asset["id"].as_str().unwrap().to_string();
        sqlx::query("UPDATE asset_exif SET latitude = 48.8, longitude = 2.3, city = 'Paris', make = 'Testcam' WHERE assetId = ?")
            .bind(&id)
            .execute(app.pool())
            .await
            .unwrap();
        ids.push(id);
    }

    let (status, body) = call(&app, json_request("GET", "/api/map/markers", None, None)).await;
    let missing: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["message"], "Authentication required");

    let (status, body) = call(&app, json_request("GET", "/api/map/markers", Some(&cookie), None)).await;
    let markers: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{markers}");
    assert_eq!(markers.as_array().unwrap().len(), 5);
    assert_eq!(markers[0]["lat"], 48.8);
    assert_eq!(markers[0]["city"], "Paris");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/map/markers?fileCreatedBefore=2010-01-01T00:00:00.000Z", Some(&cookie), None),
    )
    .await;
    let early: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{early}");
    assert_eq!(early.as_array().unwrap().len(), 0);

    let (status, body) = call(&app, json_request("GET", "/api/search/cities", Some(&cookie), None)).await;
    let cities: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{cities}");
    assert_eq!(cities.as_array().unwrap().len(), 1);
    assert_eq!(cities[0]["exifInfo"]["city"], "Paris");

    let (status, body) = call(&app, json_request("GET", "/api/search/explore", Some(&cookie), None)).await;
    let explore: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{explore}");
    assert_eq!(explore[0]["fieldName"], "exifInfo.city");
    assert_eq!(explore[0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(explore[0]["items"][0]["value"], "Paris");
    assert_eq!(explore[1]["fieldName"], "createdAt");
    assert_eq!(explore[1]["items"].as_array().unwrap().len(), 5);

    let (status, _) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/assets/{}", ids[0]),
            Some(&cookie),
            Some(r#"{"visibility":"archive"}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(&app, json_request("GET", "/api/map/markers", Some(&cookie), None)).await;
    let timeline: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{timeline}");
    assert_eq!(timeline.as_array().unwrap().len(), 4);
    let (status, body) = call(
        &app,
        json_request("GET", "/api/map/markers?isArchived=true", Some(&cookie), None),
    )
    .await;
    let archived: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{archived}");
    assert_eq!(archived.as_array().unwrap().len(), 5);
    let (status, body) = call(&app, json_request("GET", "/api/search/explore", Some(&cookie), None)).await;
    let fewer: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{fewer}");
    assert_eq!(fewer[0]["items"].as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request("POST", "/api/search/large-assets", Some(&cookie), Some(r#"{"minFileSize":0}"#)),
    )
    .await;
    let large: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{large}");
    assert_eq!(large.as_array().unwrap().len(), 5);
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/search/large-assets",
            Some(&cookie),
            Some(r#"{"minFileSize":999999999}"#),
        ),
    )
    .await;
    let none: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{none}");
    assert_eq!(none.as_array().unwrap().len(), 0);
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/search/large-assets",
            Some(&cookie),
            Some(r#"{"visibility":"locked"}"#),
        ),
    )
    .await;
    let locked: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{locked}");
    assert_eq!(locked["message"], "Elevated permission is required");

    let (status, body) = call(&app, json_request("GET", "/api/view/folder/unique-paths", Some(&cookie), None)).await;
    let paths: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{paths}");
    let path = paths[0].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request("GET", &format!("/api/view/folder?path={path}"), Some(&cookie), None),
    )
    .await;
    let folder: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{folder}");
    assert!(!folder.as_array().unwrap().is_empty());

    let (status, body) = call(
        &app,
        json_request("GET", "/api/search/suggestions?type=city&includeNull=true", Some(&cookie), None),
    )
    .await;
    let suggestions: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{suggestions}");
    assert_eq!(suggestions[0], "Paris");
    assert!(suggestions[suggestions.as_array().unwrap().len() - 1].is_null());

    let (status, body) = call(&app, json_request("GET", "/api/search/places", Some(&cookie), None)).await;
    let places: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{places}");
    assert_eq!(places["message"], "Validation failed");
    sqlx::query(
        "INSERT INTO geodata_places (id, name, alternateNames, latitude, longitude, admin1Name, admin2Name)
         VALUES (1, 'Paris', 'Lutecia', 48.8, 2.3, 'Ile-de-France', 'Paris')",
    )
    .execute(app.pool())
    .await
    .unwrap();
    let (status, body) = call(
        &app,
        json_request("GET", "/api/search/places?name=lutecia", Some(&cookie), None),
    )
    .await;
    let found: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{found}");
    assert_eq!(found[0]["name"], "Paris");
    assert_eq!(found[0]["admin1name"], "Ile-de-France");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&cookie),
            Some(&format!(r#"{{"type":"INDIVIDUAL","assetIds":["{}"]}}"#, ids[1])),
        ),
    )
    .await;
    let link: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{link}");
    let key = link["key"].as_str().unwrap();
    let (status, body) = call(
        &app,
        Request::builder()
            .method("GET")
            .uri("/api/search/explore")
            .header("x-immich-share-key", key)
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let forbidden: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 403, "{forbidden}");
    assert_eq!(forbidden["message"], "Forbidden");

    let _ = std::fs::remove_dir_all(&dir);
}
