use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::{json, Value};
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

fn jpeg(red: u8) -> Vec<u8> {
    let image = RgbImage::from_pixel(
        16 + u32::from(red),
        10 + u32::from(red),
        Rgb([red, 255 - red, 40]),
    );
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn upload(cookie: &str, bytes: &[u8], favorite: bool) -> Request<Body> {
    let boundary = "immichboundary";
    let favorite = if favorite { "true" } else { "false" };
    let mut body = Vec::new();
    for (name, value) in [
        ("deviceAssetId", "device-1"),
        ("deviceId", "phone"),
        ("fileCreatedAt", "2020-01-02T03:04:05.000Z"),
        ("fileModifiedAt", "2020-01-02T03:04:05.000Z"),
        ("isFavorite", favorite),
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

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| panic!("body {}", String::from_utf8_lossy(bytes)))
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

async fn expect(app: &App, request: Request<Body>, status: u16) -> Value {
    let (got, bytes) = call(app, request).await;
    let body = json_body(&bytes);
    assert_eq!(got, status, "{body}");
    body
}

async fn expect_empty(app: &App, request: Request<Body>, status: u16) {
    let (got, bytes) = call(app, request).await;
    assert_eq!(got, status, "{}", String::from_utf8_lossy(&bytes));
    assert!(bytes.is_empty(), "{}", String::from_utf8_lossy(&bytes));
}

async fn upload_asset(app: &App, cookie: &str, red: u8, favorite: bool) -> String {
    let asset = expect(app, upload(cookie, &jpeg(red), favorite), 201).await;
    asset["id"].as_str().unwrap().to_string()
}

async fn add_user(app: &App, cookie: &str, email: &str, name: &str) -> (String, String) {
    let body = format!(r#"{{"email":"{email}","password":"partner-secret","name":"{name}"}}"#);
    let user = expect(
        app,
        json_request("POST", "/api/admin/users", Some(cookie), Some(&body)),
        201,
    )
    .await;
    let id = user["id"].as_str().unwrap().to_string();
    let (cookie, user_id) = sign_in(app, email, "partner-secret").await;
    assert_eq!(user_id, id);
    (cookie, id)
}

fn months_of(buckets: &Value) -> Vec<String> {
    buckets
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["timeBucket"].as_str().unwrap().to_string())
        .collect()
}

fn ids_of(bucket: &Value) -> Vec<String> {
    bucket["id"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn index_of(bucket: &Value, id: &str) -> usize {
    bucket["id"]
        .as_array()
        .unwrap()
        .iter()
        .position(|item| item.as_str() == Some(id))
        .unwrap_or_else(|| panic!("{id} missing from {}", bucket["id"]))
}

async fn insert_face(
    app: &App,
    asset_id: &str,
    owner_id: &str,
    person_id: Option<&str>,
    visible: bool,
) -> String {
    let id = Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO asset_face (
            id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
            boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, isVisible,
            ownerId, assetVisibility
         ) VALUES (?, ?, ?, 16, 10, 1, 2, 8, 9, 'machine-learning', NULL, ?, ?, ?, ?, 'timeline')",
    )
    .bind(&id)
    .bind(asset_id)
    .bind(person_id)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .bind(i64::from(visible))
    .bind(owner_id)
    .execute(app.pool())
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn signed_in_timeline_filters_faces_and_ocr() {
    let dir = std::env::temp_dir().join(format!("immich-timeline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (ada, ada_id) = sign_in(&app, "admin@example.com", "correct horse").await;
    let (bea, bea_id) = add_user(&app, &ada, "bea@example.com", "Bea").await;
    let (cam, cam_id) = add_user(&app, &ada, "cam@example.com", "Cam").await;

    let january = upload_asset(&app, &ada, 1, false).await;
    let archive = upload_asset(&app, &ada, 2, false).await;
    let hidden = upload_asset(&app, &ada, 3, false).await;
    let locked = upload_asset(&app, &ada, 4, false).await;
    let favorite = upload_asset(&app, &ada, 5, true).await;
    let march = upload_asset(&app, &ada, 6, false).await;
    let stack_primary = upload_asset(&app, &ada, 7, false).await;
    let stack_child = upload_asset(&app, &ada, 8, false).await;
    let trash = upload_asset(&app, &ada, 9, false).await;
    let outside = upload_asset(&app, &ada, 10, false).await;
    let partner_photo = upload_asset(&app, &bea, 11, false).await;
    let stranger_photo = upload_asset(&app, &cam, 12, false).await;

    for (id, visibility) in [
        (&archive, "archive"),
        (&hidden, "hidden"),
        (&locked, "locked"),
    ] {
        let body = format!(r#"{{"ids":["{id}"],"visibility":"{visibility}"}}"#);
        expect_empty(
            &app,
            json_request("PUT", "/api/assets", Some(&ada), Some(&body)),
            204,
        )
        .await;
    }
    sqlx::query("UPDATE asset SET localDateTime = ? WHERE id = ?")
        .bind("2021-03-04T00:00:00.000Z")
        .bind(&march)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE asset SET createdAt = ? WHERE id = ?")
        .bind("2019-06-01T00:00:00.000Z")
        .bind(&march)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE asset_exif SET latitude = 37.5, longitude = -122.5 WHERE assetId = ?")
        .bind(&january)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE asset_exif SET latitude = 10, longitude = 10 WHERE assetId = ?")
        .bind(&outside)
        .execute(app.pool())
        .await
        .unwrap();

    let person = expect(
        &app,
        json_request("POST", "/api/people", Some(&ada), Some("{}")),
        201,
    )
    .await;
    let person_id = person["id"].as_str().unwrap().to_string();
    let visible_face = insert_face(&app, &january, &ada_id, Some(&person_id), true).await;
    let hidden_face = insert_face(&app, &january, &ada_id, Some(&person_id), false).await;
    insert_face(&app, &partner_photo, &bea_id, Some(&person_id), true).await;

    let parent = expect(
        &app,
        json_request("POST", "/api/tags", Some(&ada), Some(r#"{"name":"Trip"}"#)),
        201,
    )
    .await;
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child_body = format!(r#"{{"name":"Beach","parentId":"{parent_id}"}}"#);
    let child = expect(
        &app,
        json_request("POST", "/api/tags", Some(&ada), Some(&child_body)),
        201,
    )
    .await;
    let child_id = child["id"].as_str().unwrap();
    let tagged = expect(
        &app,
        json_request(
            "PUT",
            &format!("/api/tags/{child_id}/assets"),
            Some(&ada),
            Some(&format!(r#"{{"ids":["{january}"]}}"#)),
        ),
        200,
    )
    .await;
    assert_eq!(tagged[0]["success"], true);
    let foreign_tag = expect(
        &app,
        json_request("POST", "/api/tags", Some(&bea), Some(r#"{"name":"Bea"}"#)),
        201,
    )
    .await;
    let foreign_tag_id = foreign_tag["id"].as_str().unwrap();

    let stack = expect(
        &app,
        json_request(
            "POST",
            "/api/stacks",
            Some(&ada),
            Some(&format!(
                r#"{{"assetIds":["{stack_primary}","{stack_child}"]}}"#
            )),
        ),
        201,
    )
    .await;
    let stack_id = stack["id"].as_str().unwrap().to_string();

    let unsigned = expect(
        &app,
        json_request("GET", "/api/timeline/buckets", None, None),
        401,
    )
    .await;
    assert_eq!(unsigned["message"], "Authentication required");

    let buckets = expect(
        &app,
        json_request("GET", "/api/timeline/buckets", Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(
        months_of(&buckets),
        vec!["2021-03-01".to_string(), "2020-01-01".to_string()]
    );
    let asc = expect(
        &app,
        json_request("GET", "/api/timeline/buckets?order=asc", Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(
        months_of(&asc),
        vec!["2020-01-01".to_string(), "2021-03-01".to_string()]
    );
    let january_bucket = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    let mut expected = vec![
        january.clone(),
        archive.clone(),
        favorite.clone(),
        stack_primary.clone(),
        stack_child.clone(),
        trash.clone(),
        outside.clone(),
    ];
    assert_eq!(sorted(ids_of(&january_bucket)), sorted(expected.clone()));
    assert!(!ids_of(&january_bucket).contains(&hidden));
    assert!(!ids_of(&january_bucket).contains(&locked));
    assert!(january_bucket.get("stack").is_none());
    assert_eq!(
        january_bucket["isTrashed"][index_of(&january_bucket, &january)],
        false
    );
    assert_eq!(
        january_bucket["status"][index_of(&january_bucket, &january)],
        "active"
    );

    for query in ["order=nope", "orderBy=nope", "visibility=nope"] {
        let failed = expect(
            &app,
            json_request(
                "GET",
                &format!("/api/timeline/buckets?{query}"),
                Some(&ada),
                None,
            ),
            400,
        )
        .await;
        assert_eq!(failed["message"], "Validation failed", "{query}");
    }

    let created = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/buckets?orderBy=createdAt",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    let created_months = months_of(&created);
    assert!(created_months.contains(&"2019-06-01".to_string()));
    assert!(!created_months.contains(&"2021-03-01".to_string()));
    assert!(created_months.windows(2).all(|pair| pair[0] >= pair[1]));
    let june = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2019-06-01T00:00:00.000Z&orderBy=createdAt",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(ids_of(&june), vec![march.clone()]);

    let inside = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&bbox=-123,37,-122,38",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(ids_of(&inside), vec![january.clone()]);
    for bbox in ["1,2,3", "0,10,1,0", "200,0,10,1"] {
        let failed = expect(
            &app,
            json_request(
                "GET",
                &format!("/api/timeline/buckets?bbox={bbox}"),
                Some(&ada),
                None,
            ),
            400,
        )
        .await;
        assert_eq!(failed["message"], "Validation failed", "{bbox}");
    }

    let tagged_bucket = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&tagId={parent_id}"),
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(ids_of(&tagged_bucket), vec![january.clone()]);
    for tag_id in [foreign_tag_id, "00000000-0000-4000-8000-000000000099"] {
        let failed = expect(
            &app,
            json_request(
                "GET",
                &format!("/api/timeline/buckets?tagId={tag_id}"),
                Some(&ada),
                None,
            ),
            400,
        )
        .await;
        assert_eq!(
            failed["message"], "Not found or no tag.read access",
            "{tag_id}"
        );
    }

    let stacked = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&withStacked=true",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    let stacked_ids = ids_of(&stacked);
    assert!(stacked_ids.contains(&stack_primary));
    assert!(!stacked_ids.contains(&stack_child));
    assert!(stacked_ids.contains(&january));
    assert_eq!(
        stacked["stack"][index_of(&stacked, &stack_primary)],
        json!([stack_id, "2"])
    );
    assert!(stacked["stack"][index_of(&stacked, &january)].is_null());

    let favorites = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&isFavorite=true",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(ids_of(&favorites), vec![favorite.clone()]);
    assert_eq!(favorites["isFavorite"][0], true);

    expect(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&ada),
            Some(&format!(r#"{{"sharedWithId":"{bea_id}"}}"#)),
        ),
        201,
    )
    .await;
    expect(
        &app,
        json_request(
            "POST",
            "/api/partners",
            Some(&bea),
            Some(&format!(r#"{{"sharedWithId":"{ada_id}"}}"#)),
        ),
        201,
    )
    .await;
    let partner_view = expect(
        &app,
        json_request(
            "GET",
            &format!(
                "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&userId={ada_id}&visibility=timeline"
            ),
            Some(&bea),
            None,
        ),
        200,
    )
    .await;
    assert!(ids_of(&partner_view).contains(&favorite));
    assert_eq!(
        partner_view["isFavorite"][index_of(&partner_view, &favorite)],
        false
    );

    expect_empty(
        &app,
        json_request(
            "DELETE",
            "/api/assets",
            Some(&ada),
            Some(&format!(r#"{{"ids":["{trash}"]}}"#)),
        ),
        204,
    )
    .await;
    let after_trash = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    expected.retain(|id| id != &trash);
    assert_eq!(sorted(ids_of(&after_trash)), sorted(expected));
    let trashed = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&isTrashed=true",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(ids_of(&trashed), vec![trash.clone()]);
    assert_eq!(trashed["isTrashed"][0], true);

    let partners_rejected = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/buckets?withPartners=true",
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(
        partners_rejected["message"],
        "withPartners is only supported for non-archived, non-trashed, non-favorited, non-locked assets"
    );
    let stranger = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/timeline/buckets?userId={cam_id}"),
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(stranger["message"], "Not found or no timeline.read access");
    let partner_archive = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/timeline/buckets?userId={bea_id}&visibility=archive"),
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(
        partner_archive["message"],
        "Not found or no archive.read access"
    );
    let person_timeline = expect(
        &app,
        json_request(
            "GET",
            &format!(
                "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&visibility=timeline&withPartners=true&personId={person_id}"
            ),
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    let person_ids = ids_of(&person_timeline);
    assert!(person_ids.contains(&january));
    assert!(person_ids.contains(&partner_photo));
    let main_partners = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/bucket?timeBucket=2020-01-01T00:00:00.000Z&visibility=timeline&withPartners=true",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert!(!ids_of(&main_partners).contains(&partner_photo));
    let missing_person = expect(
        &app,
        json_request(
            "GET",
            "/api/timeline/buckets?visibility=timeline&personId=00000000-0000-4000-8000-000000000088",
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(missing_person, json!([]));

    let asset = expect(
        &app,
        json_request("GET", &format!("/api/assets/{january}"), Some(&ada), None),
        200,
    )
    .await;
    let owner = asset["owner"].as_object().unwrap();
    let mut owner_keys: Vec<_> = owner.keys().cloned().collect();
    owner_keys.sort();
    assert_eq!(
        owner_keys,
        vec![
            "avatarColor",
            "email",
            "id",
            "name",
            "profileChangedAt",
            "profileImagePath",
        ]
    );
    assert_eq!(asset["owner"]["id"], ada_id);
    assert_eq!(asset["owner"]["email"], "admin@example.com");
    assert_eq!(asset["owner"]["name"], "Ada");
    assert_eq!(asset["edits"], json!([]));
    assert!(asset["exifInfo"].is_object());
    assert_eq!(asset["exifInfo"]["latitude"], 37.5);
    assert_eq!(asset["exifInfo"]["longitude"], -122.5);
    assert_eq!(asset["people"][0]["id"], person_id);
    let missing_asset = expect(
        &app,
        json_request(
            "GET",
            "/api/assets/00000000-0000-4000-8000-000000000099",
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(missing_asset["message"], "Asset not found");
    let unreadable = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{stranger_photo}"),
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(unreadable["message"], "Not found or no asset.read access");

    let now = "2020-01-02T03:04:05.000Z";
    let ocr_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO asset_ocr (
            id, assetId, x1, y1, x2, y2, x3, y3, x4, y4, boxScore, textScore, text, isVisible,
            updatedAt, updateId
         ) VALUES (?, ?, 1, 2, 3, 2, 3, 4, 1, 4, 0.5, 0.25, 'hello', 1, ?, ?)",
    )
    .bind(&ocr_id)
    .bind(&january)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO asset_ocr (
            id, assetId, x1, y1, x2, y2, x3, y3, x4, y4, boxScore, textScore, text, isVisible,
            updatedAt, updateId
         ) VALUES (?, ?, 0, 0, 1, 0, 1, 1, 0, 1, 0.5, 0.25, 'secret', 0, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&january)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    let regions = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{january}/ocr"),
            Some(&ada),
            None,
        ),
        200,
    )
    .await;
    assert_eq!(regions.as_array().unwrap().len(), 1);
    assert_eq!(regions[0]["id"], ocr_id);
    assert_eq!(regions[0]["assetId"], january);
    assert_eq!(regions[0]["text"], "hello");
    assert_eq!(regions[0]["textScore"], 0.25);
    assert_eq!(regions[0]["boxScore"], 0.5);
    assert_eq!(regions[0]["x1"], 1.0);
    assert_eq!(regions[0]["y4"], 4.0);
    let ocr_missing = expect(
        &app,
        json_request(
            "GET",
            "/api/assets/00000000-0000-4000-8000-000000000099/ocr",
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(ocr_missing["message"], "Asset not found");
    let ocr_invalid = expect(
        &app,
        json_request("GET", "/api/assets/not-a-uuid/ocr", Some(&ada), None),
        400,
    )
    .await;
    assert_eq!(ocr_invalid["message"], "Validation failed");
    let ocr_denied = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{stranger_photo}/ocr"),
            Some(&ada),
            None,
        ),
        400,
    )
    .await;
    assert_eq!(ocr_denied["message"], "Not found or no asset.read access");

    let faces = expect(
        &app,
        json_request("GET", &format!("/api/faces?id={january}"), Some(&ada), None),
        200,
    )
    .await;
    assert_eq!(faces.as_array().unwrap().len(), 1);
    assert_eq!(faces[0]["id"], visible_face);
    assert_ne!(faces[0]["id"], hidden_face);
    assert_eq!(faces[0]["imageWidth"], 16);
    assert_eq!(faces[0]["imageHeight"], 10);
    assert_eq!(faces[0]["boundingBoxX1"], 1);
    assert_eq!(faces[0]["boundingBoxY1"], 2);
    assert_eq!(faces[0]["boundingBoxX2"], 8);
    assert_eq!(faces[0]["boundingBoxY2"], 9);
    assert_eq!(faces[0]["sourceType"], "machine-learning");
    assert_eq!(faces[0]["person"]["id"], person_id);
    let faces_missing_id = expect(
        &app,
        json_request("GET", "/api/faces", Some(&ada), None),
        400,
    )
    .await;
    assert_eq!(faces_missing_id["message"], "Validation failed");
    let faces_invalid = expect(
        &app,
        json_request("GET", "/api/faces?id=not-a-uuid", Some(&ada), None),
        400,
    )
    .await;
    assert_eq!(faces_invalid["message"], "Validation failed");
    let share = "immich_shared_link_token=abc";
    let faces_share = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/faces?id={january}"),
            Some(share),
            None,
        ),
        403,
    )
    .await;
    assert_eq!(faces_share["message"], "Forbidden");
    let ocr_share = expect(
        &app,
        json_request(
            "GET",
            &format!("/api/assets/{january}/ocr"),
            Some(share),
            None,
        ),
        403,
    )
    .await;
    assert_eq!(ocr_share["message"], "Forbidden");
}
