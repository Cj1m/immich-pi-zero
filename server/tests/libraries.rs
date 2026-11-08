//! External libraries match the v3.3.6 documents. A scan does not import a folder.

use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

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

fn message(bytes: &[u8]) -> String {
    json_body(bytes)["message"]
        .as_str()
        .unwrap_or("")
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
    if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        return builder.body(Body::from(body.to_string())).unwrap();
    }
    builder.body(Body::empty()).unwrap()
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

fn jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(8, 6, Rgb([20, 40, 60]));
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
        ("deviceAssetId", "library-device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("filename", "cat.jpg"),
    ] {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .into_bytes(),
        );
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"cat.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n"
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

fn set_mode(path: &Path, mode: u32) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    std::fs::set_permissions(path, permissions).unwrap();
}

async fn insert_library_asset(
    app: &App,
    owner: &str,
    library_id: &str,
    kind: &str,
    visibility: &str,
    deleted_at: Option<&str>,
    size: i64,
) {
    let id = Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO asset (
            id, ownerId, type, originalPath, originalFileName, originalMimeType,
            fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
            libraryId, deletedAt, updateId, createdAt, updatedAt
         ) VALUES (?, ?, ?, ?, 'photo.jpg', 'image/jpeg', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner)
    .bind(kind)
    .bind(format!("{owner}/{id}/original"))
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(id.as_bytes())
    .bind(visibility)
    .bind(library_id)
    .bind(deleted_at)
    .bind(Uuid::now_v7().to_string())
    .bind(now)
    .bind(now)
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO asset_exif (assetId, fileSizeInByte, description, updatedAt, updateId)
         VALUES (?, ?, '', ?, ?)",
    )
    .bind(&id)
    .bind(size)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn external_libraries_do_not_import() {
    let dir = std::env::temp_dir().join(format!("immich-libraries-{}", std::process::id()));
    let import_dir =
        std::env::temp_dir().join(format!("immich-libraries-import-{}", std::process::id()));
    let prefix_dir =
        std::env::temp_dir().join(format!("immich-libraries-{}-extra", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&import_dir);
    let _ = std::fs::remove_dir_all(&prefix_dir);
    std::fs::create_dir_all(&import_dir).unwrap();
    std::fs::create_dir_all(&prefix_dir).unwrap();
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (admin, admin_id) = sign_in(&app, "admin@example.com", "correct horse").await;

    let (status, bytes) = call(&app, json_request("GET", "/api/libraries", None, None)).await;
    assert_eq!(status, 401, "{}", message(&bytes));
    assert_eq!(message(&bytes), "Authentication required");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            "/api/libraries",
            Some("immich_shared_link_token=link"),
            None,
        ),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(message(&bytes), "Forbidden");

    let guest_body = r#"{"email":"bea@example.com","password":"temp-pass","name":"Bea"}"#;
    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/admin/users", Some(&admin), Some(guest_body)),
    )
    .await;
    assert_eq!(status, 201, "{}", message(&bytes));
    let (guest, _) = sign_in(&app, "bea@example.com", "temp-pass").await;

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/libraries/not-a-uuid", Some(&guest), None),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(message(&bytes), "Forbidden");

    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/libraries", Some(&guest), Some("not-json")),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(message(&bytes), "Forbidden");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/libraries?key=abc", Some(&admin), None),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(message(&bytes), "Invalid share key");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/libraries", Some(&admin), None),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json_body(&bytes), serde_json::json!([]));

    let (status, bytes) = call(
        &app,
        json_request("POST", "/api/libraries", Some(&admin), Some("{}")),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let unknown = Uuid::new_v4();
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/libraries",
            Some(&admin),
            Some(&format!(r#"{{"ownerId":"{unknown}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/libraries",
            Some(&admin),
            Some(&format!(
                r#"{{"ownerId":"{admin_id}","importPaths":["/tmp/a","/tmp/a"]}}"#
            )),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/libraries",
            Some(&admin),
            Some(&format!(r#"{{"ownerId":"{admin_id}"}}"#)),
        ),
    )
    .await;
    let created = json_body(&bytes);
    assert_eq!(status, 201, "{created}");
    let library_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["ownerId"], admin_id);
    assert_eq!(created["name"], "New External Library");
    assert_eq!(created["assetCount"], 0);
    assert_eq!(created["importPaths"], serde_json::json!([]));
    assert_eq!(
        created["exclusionPatterns"],
        serde_json::json!([
            "**/@eaDir/**",
            "**/._*",
            "**/#recycle/**",
            "**/#snapshot/**",
            "**/.stversions/**",
            "**/.stfolder/**"
        ])
    );
    assert!(created["refreshedAt"].is_null());
    assert!(created["createdAt"].as_str().unwrap().ends_with('Z'));
    assert_eq!(created["createdAt"], created["updatedAt"]);

    let gone_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO library (
            id, name, ownerId, importPaths, exclusionPatterns, createdAt, updatedAt, deletedAt, updateId
         ) VALUES (?, 'Gone', ?, '[]', '[]', ?, ?, ?, ?)",
    )
    .bind(&gone_id)
    .bind(&admin_id)
    .bind(created["createdAt"].as_str().unwrap())
    .bind(created["createdAt"].as_str().unwrap())
    .bind(created["createdAt"].as_str().unwrap())
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/libraries", Some(&admin), None),
    )
    .await;
    let list = json_body(&bytes);
    assert_eq!(status, 200, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], library_id);

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/libraries/not-a-uuid", Some(&admin), None),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let missing = Uuid::new_v4();
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{missing}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Library not found");

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{gone_id}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Library not found");

    let relative = std::path::absolute("relative/path").unwrap();
    let missing_path =
        std::env::temp_dir().join(format!("immich-libraries-missing-{}", std::process::id()));
    let file_path = import_dir.join("note.txt");
    std::fs::write(&file_path, b"not a directory").unwrap();
    let locked = import_dir.join("locked");
    std::fs::create_dir(&locked).unwrap();
    set_mode(&locked, 0o000);
    let validate_body = serde_json::json!({
        "importPaths": [
            dir,
            dir.join("objects"),
            "relative/path",
            missing_path,
            file_path,
            locked,
            import_dir,
            prefix_dir
        ],
        "exclusionPatterns": ["**/.stfolder/**"]
    })
    .to_string();
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{missing}/validate"),
            Some(&admin),
            Some(&validate_body),
        ),
    )
    .await;
    set_mode(&locked, 0o755);
    let validated = json_body(&bytes);
    assert_eq!(status, 200, "{validated}");
    let checks = validated["importPaths"].as_array().unwrap();
    assert_eq!(checks.len(), 8);
    assert_eq!(
        checks[0]["message"],
        "Cannot use media upload folder for external libraries"
    );
    assert_eq!(
        checks[1]["message"],
        "Cannot use media upload folder for external libraries"
    );
    assert_eq!(checks[0]["isValid"], false);
    assert_eq!(
        checks[2]["message"],
        format!("Import path must be absolute, try {}", relative.display())
    );
    assert_eq!(checks[3]["message"], "Path does not exist (ENOENT)");
    assert_eq!(checks[4]["message"], "Not a directory");
    assert_eq!(checks[5]["message"], "Lacking read permission for folder");
    assert_eq!(checks[6]["isValid"], true);
    assert!(checks[6].get("message").is_none());
    assert_eq!(
        checks[6]["importPath"],
        import_dir.to_string_lossy().as_ref()
    );
    assert_eq!(checks[7]["isValid"], true);
    assert!(checks[7].get("message").is_none());

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{missing}/validate"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json_body(&bytes)["importPaths"], serde_json::json!([]));

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{missing}/validate"),
            Some(&admin),
            Some("null"),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json_body(&bytes)["importPaths"], serde_json::json!([]));

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{missing}/validate"),
            Some(&admin),
            Some("[]"),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{missing}/validate"),
            Some(&admin),
            Some(r#"{"exclusionPatterns":["a","a"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            "/api/libraries/not-a-uuid/validate",
            Some(&admin),
            Some("{}"),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            Some(r#"{"name":""}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Validation failed");

    let put_message = format!(
        "Invalid import path: Import path must be absolute, try {}",
        relative.display()
    );
    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            Some(r#"{"importPaths":["relative/path"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), put_message);

    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            Some("{}"),
        ),
    )
    .await;
    let unchanged = json_body(&bytes);
    assert_eq!(status, 200, "{unchanged}");
    assert_eq!(unchanged["name"], "New External Library");
    assert_eq!(unchanged["importPaths"], serde_json::json!([]));
    assert_eq!(unchanged["updatedAt"], created["updatedAt"]);

    let import_text = import_dir.to_string_lossy().to_string();
    let (status, bytes) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            Some(&serde_json::json!({ "importPaths": [import_text] }).to_string()),
        ),
    )
    .await;
    let updated = json_body(&bytes);
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["importPaths"], serde_json::json!([import_text]));
    assert_ne!(updated["updatedAt"], created["updatedAt"]);

    let (status, bytes) = call(
        &app,
        json_request(
            "PATCH",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            Some(r#"{"name":"Camera roll"}"#),
        ),
    )
    .await;
    let patched = json_body(&bytes);
    assert_eq!(status, 200, "{patched}");
    assert_eq!(patched["name"], "Camera roll");
    assert_eq!(patched["importPaths"], serde_json::json!([import_text]));

    let photo = import_dir.join("roll.jpg");
    std::fs::write(&photo, jpeg()).unwrap();
    let assets_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset")
        .fetch_one(app.pool())
        .await
        .unwrap();
    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{library_id}/scan"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));
    assert!(photo.is_file());
    let assets_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(assets_after, assets_before);
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    let after_scan = json_body(&bytes);
    assert_eq!(status, 200, "{after_scan}");
    assert_eq!(after_scan["assetCount"], 0);
    assert!(after_scan["refreshedAt"].is_null());

    let (status, bytes) = call(&app, upload(&admin, &jpeg())).await;
    let uploaded = json_body(&bytes);
    assert_eq!(status, 201, "{uploaded}");
    assert!(uploaded["libraryId"].is_null());
    let asset_id = uploaded["id"].as_str().unwrap().to_string();
    let stored: Option<String> = sqlx::query_scalar("SELECT libraryId FROM asset WHERE id = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert!(stored.is_none());

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{library_id}/statistics"),
            Some(&admin),
            None,
        ),
    )
    .await;
    let empty_stats = json_body(&bytes);
    assert_eq!(status, 200, "{empty_stats}");
    assert_eq!(empty_stats["photos"], 0);
    assert_eq!(empty_stats["videos"], 0);
    assert_eq!(empty_stats["total"], 0);
    assert_eq!(empty_stats["usage"], 0);
    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", message(&bytes));
    assert_eq!(json_body(&bytes)["assetCount"], 0);

    sqlx::query("UPDATE asset SET libraryId = ? WHERE id = ?")
        .bind(&library_id)
        .bind(&asset_id)
        .execute(app.pool())
        .await
        .unwrap();
    let uploaded_size: i64 =
        sqlx::query_scalar("SELECT fileSizeInByte FROM asset_exif WHERE assetId = ?")
            .bind(&asset_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    insert_library_asset(&app, &admin_id, &library_id, "VIDEO", "hidden", None, 9).await;
    insert_library_asset(
        &app,
        &admin_id,
        &library_id,
        "IMAGE",
        "timeline",
        Some("2020-01-03T00:00:00.000Z"),
        1000,
    )
    .await;
    insert_library_asset(&app, &admin_id, &library_id, "VIDEO", "timeline", None, 40).await;

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json_body(&bytes)["assetCount"], 3);

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{library_id}/statistics"),
            Some(&admin),
            None,
        ),
    )
    .await;
    let stats = json_body(&bytes);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["photos"], 1);
    assert_eq!(stats["videos"], 1);
    assert_eq!(stats["total"], 2);
    assert_eq!(stats["usage"], uploaded_size + 9 + 40);

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{missing}/statistics"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), format!("Library {missing} not found"));

    let (status, bytes) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/libraries/{missing}/scan"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Library not found");

    let (status, bytes) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&bytes));

    let (status, bytes) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/libraries/{library_id}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&bytes), "Library not found");

    let (status, bytes) = call(
        &app,
        json_request("GET", "/api/libraries", Some(&admin), None),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json_body(&bytes), serde_json::json!([]));

    let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM asset WHERE id = ?")
        .bind(&asset_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(still_there, 1);
    assert!(photo.is_file());

    set_mode(&locked, 0o755);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&import_dir);
    let _ = std::fs::remove_dir_all(&prefix_dir);
}
