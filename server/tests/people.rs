use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use image::{ImageFormat, Rgb, RgbImage};
use immich_server::{App, Config};
use serde_json::{json, Value};
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

async fn insert_person(
    app: &App,
    owner: &str,
    asset: &str,
    name: &str,
    hidden: i64,
    faces: i64,
) -> String {
    let person_id = uuid::Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    let update_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
         VALUES (?, NULL, ?, ?, ?, ?)",
    )
    .bind(&person_id)
    .bind(now)
    .bind(&update_id)
    .bind(now)
    .bind(&update_id)
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO person
            (ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden, birthDate,
             faceAssetId, isFavorite, color, updateId)
         VALUES (?, ?, ?, ?, ?, '', ?, NULL, NULL, 0, NULL, ?)",
    )
    .bind(owner)
    .bind(&person_id)
    .bind(now)
    .bind(now)
    .bind(name)
    .bind(hidden)
    .bind(uuid::Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    for index in 0..faces {
        sqlx::query(
            "INSERT INTO asset_face
                (id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
                 boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, isVisible,
                 ownerId, assetVisibility, assetDeletedAt)
             VALUES (?, ?, ?, 16, 10, ?, 0, ?, 4, 'machine-learning', NULL, ?, ?, 1, ?, 'timeline', NULL)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(asset)
        .bind(&person_id)
        .bind(index)
        .bind(index + 2)
        .bind(now)
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(owner)
        .execute(app.pool())
        .await
        .unwrap();
    }
    person_id
}

#[tokio::test]
async fn people_list_search_and_shares() {
    let dir = std::env::temp_dir().join(format!("immich-people-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"email":"admin@example.com","password":"correct horse"}"#),
        ),
    )
    .await;
    let login: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{login}");
    let user_id = login["userId"].as_str().unwrap().to_string();
    let cookie = {
        let response = immich_server::router(app.clone())
            .oneshot(json_request(
                "POST",
                "/api/auth/login",
                None,
                Some(r#"{"email":"admin@example.com","password":"correct horse"}"#),
            ))
            .await
            .unwrap();
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
    };

    let (status, body) = call(&app, json_request("GET", "/api/people", None, None)).await;
    let missing: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["message"], "Authentication required");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/people", Some(&cookie), None),
    )
    .await;
    let empty: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty["people"].as_array().unwrap().len(), 0);
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["hidden"], 0);
    assert_eq!(empty["hasNextPage"], false);

    let (status, body) = call(&app, upload(&cookie, &jpeg(64))).await;
    let asset: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{asset}");
    let asset_id = asset["id"].as_str().unwrap();

    let ada = insert_person(&app, &user_id, asset_id, "Ada", 0, 1).await;
    let _hidden = insert_person(&app, &user_id, asset_id, "Hidden", 1, 1).await;
    let _ghost = insert_person(&app, &user_id, asset_id, "", 0, 1).await;
    let crowd = insert_person(&app, &user_id, asset_id, "", 0, 3).await;

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/people?withHidden=true&size=1",
            Some(&cookie),
            None,
        ),
    )
    .await;
    let page: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["total"], 3);
    assert_eq!(page["hidden"], 1);
    assert_eq!(page["hasNextPage"], true);
    assert_eq!(page["people"][0]["id"], ada);
    assert_eq!(page["people"][0]["name"], "Ada");
    assert_eq!(page["people"][0]["isHidden"], false);
    assert_eq!(page["people"].as_array().unwrap().len(), 1);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/people?withHidden=false", Some(&cookie), None),
    )
    .await;
    let visible: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{visible}");
    assert_eq!(visible["total"], 3);
    assert_eq!(visible["hidden"], 1);
    let ids: Vec<&str> = visible["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|person| person["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![ada.as_str(), crowd.as_str()]);

    let (status, body) = call(
        &app,
        json_request("GET", &format!("/api/people/{ada}"), Some(&cookie), None),
    )
    .await;
    let one: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["id"], ada);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/people/not-a-uuid", Some(&cookie), None),
    )
    .await;
    let bad_id: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{bad_id}");
    assert_eq!(bad_id["message"], "Validation failed");

    let missing_id = uuid::Uuid::new_v4();
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/people/{missing_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let absent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{absent}");
    assert_eq!(absent["message"], "Not found or no person.read access");

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/people/{ada}/statistics"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let stats: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["assets"], 1);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/people/{ada}/thumbnail"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let missing_thumb: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 404, "{missing_thumb}");
    assert_eq!(missing_thumb["message"], "Not found");
    let thumb_path = dir.join("ada.jpg");
    std::fs::write(&thumb_path, jpeg(128)).unwrap();
    sqlx::query("UPDATE person SET thumbnailPath = ? WHERE ownerId = ? AND personGroupId = ?")
        .bind(thumb_path.to_str().unwrap())
        .bind(&user_id)
        .bind(&ada)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, thumb) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/people/{ada}/thumbnail?c=1"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&thumb));
    assert_eq!(&thumb[..2], &[0xFF, 0xD8]);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/search/person?name=ada", Some(&cookie), None),
    )
    .await;
    let found: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{found}");
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["id"], ada);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/search/person", Some(&cookie), None),
    )
    .await;
    let no_name: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{no_name}");
    assert_eq!(no_name["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/people/{ada}"),
            Some(&cookie),
            Some(r#"{"isFavorite":true,"name":"Ada Lovelace","birthDate":"1815-12-10"}"#),
        ),
    )
    .await;
    let updated: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["isFavorite"], true);
    assert_eq!(updated["name"], "Ada Lovelace");
    assert_eq!(updated["birthDate"], "1815-12-10");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/people/{ada}"),
            Some(&cookie),
            Some("{}"),
        ),
    )
    .await;
    let blank: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{blank}");
    assert_eq!(blank["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/people/{ada}"),
            Some(&cookie),
            Some(r#"{"birthDate":"2999-01-01"}"#),
        ),
    )
    .await;
    let future: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{future}");
    assert_eq!(future["message"], "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/shared-links",
            Some(&cookie),
            Some(&format!(
                r#"{{"type":"INDIVIDUAL","assetIds":["{asset_id}"]}}"#
            )),
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
            .uri("/api/people")
            .header("cookie", &cookie)
            .header("x-immich-share-key", key)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let forbidden: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 403, "{forbidden}");
    assert_eq!(forbidden["message"], "Forbidden");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/people/users", Some(&cookie), None),
    )
    .await;
    let shares: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{shares}");
    assert_eq!(shares.as_array().unwrap().len(), 0);

    let other = uuid::Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO user (id, email, createdAt, updatedAt, name, profileChangedAt, updateId)
         VALUES (?, 'bea@example.com', ?, ?, 'Bea', ?, ?)",
    )
    .bind(&other)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(app.pool())
    .await
    .unwrap();

    let self_body = json!({
        "sharedWithIds": [user_id],
        "role": "read",
        "personIds": [ada],
    })
    .to_string();
    let (status, body) = call(
        &app,
        json_request("PUT", "/api/people/users", Some(&cookie), Some(&self_body)),
    )
    .await;
    let self_share: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{self_share}");
    assert_eq!(self_share["message"], "Cannot share a person with yourself");

    let other_body = json!({
        "sharedWithIds": [other],
        "role": "read",
        "personIds": [ada],
    })
    .to_string();
    let (status, body) = call(
        &app,
        json_request("PUT", "/api/people/users", Some(&cookie), Some(&other_body)),
    )
    .await;
    let no_cluster: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{no_cluster}");
    assert_eq!(
        no_cluster["message"],
        "All users must be in the same cluster group"
    );

    let cluster = uuid::Uuid::new_v4().to_string();
    sqlx::query("UPDATE user SET clusterGroupId = ? WHERE id = ? OR id = ?")
        .bind(&cluster)
        .bind(&user_id)
        .bind(&other)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, body) = call(
        &app,
        json_request("PUT", "/api/people/users", Some(&cookie), Some(&other_body)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(body.is_empty());

    let (status, body) = call(
        &app,
        json_request("GET", "/api/people/users", Some(&cookie), None),
    )
    .await;
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["personId"], ada);
    assert_eq!(listed[0]["sharedWithId"], other);
    assert_eq!(listed[0]["role"], "read");
    assert_eq!(listed[0]["sharedWith"]["name"], "Bea");

    let remove = json!([{ "personId": ada, "sharedWithId": other }]).to_string();
    let (status, body) = call(
        &app,
        json_request("DELETE", "/api/people/users", Some(&cookie), Some(&remove)),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(
        &app,
        json_request("GET", "/api/people/users", Some(&cookie), None),
    )
    .await;
    let cleared: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared.as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["AssetFacesV1"]}"#),
        ),
    )
    .await;
    let deprecated: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{deprecated}");
    assert_eq!(
        deprecated["message"],
        "SyncRequestType.AssetFacesV1 is deprecated, use SyncRequestType.AssetFacesV2 instead"
    );

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["PeopleV1","AssetFacesV3"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let lines: Vec<Value> = String::from_utf8(body)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let people: Vec<&Value> = lines
        .iter()
        .filter(|line| line["type"] == "PersonV1")
        .collect();
    assert_eq!(people.len(), 4);
    let ada_line = people
        .iter()
        .find(|line| line["data"]["id"] == ada)
        .unwrap();
    assert_eq!(ada_line["data"]["name"], "Ada Lovelace");
    assert_eq!(ada_line["data"]["birthDate"], "1815-12-10T00:00:00.000Z");
    assert_eq!(ada_line["data"]["isFavorite"], true);
    assert_eq!(ada_line["data"]["isHidden"], false);
    assert_eq!(ada_line["data"]["ownerId"], user_id);
    assert_eq!(ada_line["data"]["color"], Value::Null);
    assert_eq!(ada_line["data"]["faceAssetId"], Value::Null);
    let faces: Vec<&Value> = lines
        .iter()
        .filter(|line| line["type"] == "AssetFaceV3")
        .collect();
    assert_eq!(faces.len(), 6);
    assert!(faces
        .iter()
        .any(|line| line["data"]["personId"] == ada && line["data"]["isVisible"] == true));
    assert!(lines.iter().any(|line| line["type"] == "SyncCompleteV1"));

    let person_ack = people.last().unwrap()["ack"].as_str().unwrap();
    let face_ack = faces.last().unwrap()["ack"].as_str().unwrap();
    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/ack",
            Some(&cookie),
            Some(&format!(r#"{{"acks":["{person_ack}","{face_ack}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["PeopleV1","AssetFacesV3"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let again = String::from_utf8(body).unwrap();
    assert!(!again.contains("PersonV1"));
    assert!(!again.contains("AssetFaceV3"));
    assert!(again.contains("SyncCompleteV1"));

    let _ = std::fs::remove_dir_all(&dir);
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
    let login: Value = serde_json::from_slice(&bytes).unwrap();
    (cookie, login["userId"].as_str().unwrap().to_string())
}

async fn add_person(
    app: &App,
    owner: &str,
    asset: &str,
    name: &str,
    birth: Option<&str>,
    faces: i64,
    thumb: Option<&str>,
) -> (String, Vec<String>) {
    let person_id = uuid::Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    let update_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
         VALUES (?, NULL, ?, ?, ?, ?)",
    )
    .bind(&person_id)
    .bind(now)
    .bind(&update_id)
    .bind(now)
    .bind(&update_id)
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO person
            (ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden, birthDate,
             faceAssetId, isFavorite, color, updateId)
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, NULL, 0, NULL, ?)",
    )
    .bind(owner)
    .bind(&person_id)
    .bind(now)
    .bind(now)
    .bind(name)
    .bind(thumb.unwrap_or(""))
    .bind(birth)
    .bind(uuid::Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    let mut face_ids = Vec::new();
    for index in 0..faces {
        let face_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO asset_face
                (id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
                 boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, isVisible,
                 ownerId, assetVisibility, assetDeletedAt)
             VALUES (?, ?, ?, 80, 60, ?, 6, ?, 36, 'machine-learning', NULL, ?, ?, 1, ?, 'timeline', NULL)",
        )
        .bind(&face_id)
        .bind(asset)
        .bind(&person_id)
        .bind(8 + index * 4)
        .bind(40 + index * 4)
        .bind(now)
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(owner)
        .execute(app.pool())
        .await
        .unwrap();
        face_ids.push(face_id);
    }
    (person_id, face_ids)
}

async fn add_copy(app: &App, owner: &str, person_id: &str, name: &str, birth: Option<&str>) {
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO person
            (ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden, birthDate,
             faceAssetId, isFavorite, color, updateId)
         VALUES (?, ?, ?, ?, ?, '', 0, ?, NULL, 0, NULL, ?)",
    )
    .bind(owner)
    .bind(person_id)
    .bind(now)
    .bind(now)
    .bind(name)
    .bind(birth)
    .bind(uuid::Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
}

fn sync_lines(body: &[u8]) -> Vec<Value> {
    String::from_utf8(body.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn merge_reassign_and_delete_a_face() {
    let dir = std::env::temp_dir().join(format!("immich-faces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let (cookie, user_id) = sign_in(&app, "admin@example.com", "correct horse").await;

    let image = {
        let picture = image::RgbImage::from_pixel(80, 60, image::Rgb([210, 40, 20]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(picture)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        bytes
    };
    let (status, body) = call(&app, upload(&cookie, &image)).await;
    let uploaded: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{uploaded}");
    let asset_id = uploaded["id"].as_str().unwrap().to_string();

    let (status, body) = call(&app, json_request("POST", "/api/people", None, Some("{}"))).await;
    let unsigned: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 401, "{unsigned}");
    assert_eq!(unsigned["message"], "Authentication required");

    let (status, body) = call(
        &app,
        json_request("POST", "/api/people", Some(&cookie), Some("{}")),
    )
    .await;
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["name"], "");
    assert_eq!(created["isHidden"], false);
    assert_eq!(created["isFavorite"], false);
    assert_eq!(created["thumbnailPath"], "");
    let fresh = created["id"].as_str().unwrap().to_string();

    let thumb = dir.join("bob-face.jpeg");
    std::fs::write(&thumb, &image).unwrap();
    let (ada, _) = add_person(&app, &user_id, &asset_id, "Ada", None, 2, None).await;
    let (bob, bob_faces) = add_person(
        &app,
        &user_id,
        &asset_id,
        "Bob",
        Some("1900-01-01"),
        2,
        Some(thumb.to_str().unwrap()),
    )
    .await;

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&cookie),
            Some(r#"{"email":"guest@example.com","password":"temp-pass","name":"Guest"}"#),
        ),
    )
    .await;
    let guest: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{guest}");
    let guest_id = guest["id"].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&cookie),
            Some(r#"{"email":"mover@example.com","password":"temp-pass","name":"Mover"}"#),
        ),
    )
    .await;
    let mover: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 201, "{mover}");
    let mover_id = mover["id"].as_str().unwrap();
    add_copy(&app, guest_id, &ada, "Guest Ada", Some("1900-01-01")).await;
    add_copy(&app, guest_id, &bob, "Guest Bob", Some("1900-01-01")).await;
    add_copy(&app, mover_id, &bob, "", None).await;

    let (guest_cookie, _) = sign_in(&app, "guest@example.com", "temp-pass").await;
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/people/merge",
            Some(&guest_cookie),
            Some(
                r#"{"ids":["00000000-0000-4000-8000-000000000001","00000000-0000-4000-8000-000000000002"]}"#,
            ),
        ),
    )
    .await;
    let denied: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{denied}");
    assert_eq!(denied[0]["success"], false);
    assert_eq!(denied[0]["error"], "no_permission");
    assert_eq!(denied[1]["error"], "no_permission");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/people/merge",
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{ada}"]}}"#)),
        ),
    )
    .await;
    let short: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{short}");
    assert_eq!(
        short["message"],
        "At least two people are required for merging"
    );

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/people/merge",
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{ada}","{ada}"]}}"#)),
        ),
    )
    .await;
    let repeated: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{repeated}");
    assert_eq!(repeated["message"], "Cannot merge a person into themselves");

    let unknown = "00000000-0000-4000-8000-000000000099";
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/people/merge",
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{unknown}","{ada}"]}}"#)),
        ),
    )
    .await;
    let skipped: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{skipped}");
    assert_eq!(skipped[0]["id"], unknown);
    assert_eq!(skipped[0]["error"], "no_permission");
    assert_eq!(skipped[1]["id"], ada);
    assert_eq!(skipped[1]["success"], true);
    assert!(skipped[1].get("error").is_none());

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/people/merge",
            Some(&cookie),
            Some(&format!(r#"{{"ids":["{ada}","{bob}"]}}"#)),
        ),
    )
    .await;
    let merged: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{merged}");
    assert_eq!(merged[0]["id"], ada);
    assert_eq!(merged[0]["success"], true);
    assert_eq!(merged[1]["id"], bob);
    assert_eq!(merged[1]["success"], true);
    assert!(merged[1].get("error").is_none());
    assert!(!thumb.exists());

    let (status, body) = call(
        &app,
        json_request("GET", &format!("/api/people/{ada}"), Some(&cookie), None),
    )
    .await;
    let kept: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{kept}");
    assert_eq!(kept["name"], "Ada");
    assert_eq!(kept["birthDate"], "1900-01-01");
    let bob_faces_left: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM asset_face WHERE personGroupId = ? AND ownerId = ?",
    )
    .bind(&bob)
    .bind(&user_id)
    .fetch_one(app.pool())
    .await
    .unwrap();
    assert_eq!(bob_faces_left, 0);
    let guest_bob: String =
        sqlx::query_scalar("SELECT name FROM person WHERE ownerId = ? AND personGroupId = ?")
            .bind(guest_id)
            .bind(&bob)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(guest_bob, "Guest Bob");
    let mover_group: String =
        sqlx::query_scalar("SELECT personGroupId FROM person WHERE ownerId = ?")
            .bind(mover_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(mover_group, ada);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["PeopleV1","AssetFacesV3"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let lines = sync_lines(&body);
    assert!(lines
        .iter()
        .any(|line| { line["type"] == "PersonDeleteV1" && line["data"]["personId"] == bob }));
    assert!(lines.iter().any(|line| {
        line["type"] == "AssetFaceV3"
            && line["data"]["id"] == bob_faces[0]
            && line["data"]["personId"] == ada
    }));

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/people/{fresh}/reassign"),
            Some(&cookie),
            Some(&format!(
                r#"{{"data":[{{"assetId":"{asset_id}","personId":"{ada}"}}]}}"#
            )),
        ),
    )
    .await;
    let moved: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved.as_array().unwrap().len(), 1);
    assert_eq!(moved[0]["id"], fresh);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/people/{fresh}/thumbnail"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(&body[..2], &[0xFF, 0xD8]);

    let missing = "00000000-0000-4000-8000-000000000077";
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/people/{missing}/reassign"),
            Some(&cookie),
            Some(r#"{"data":[]}"#),
        ),
    )
    .await;
    let absent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{absent}");
    assert_eq!(absent["message"], "Person not found");

    let face_id: String = sqlx::query_scalar(
        "SELECT id FROM asset_face WHERE personGroupId = ? AND ownerId = ? ORDER BY id LIMIT 1",
    )
    .bind(&fresh)
    .bind(&user_id)
    .fetch_one(app.pool())
    .await
    .unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/faces/{ada}"),
            Some(&cookie),
            Some(&format!(r#"{{"id":"{face_id}"}}"#)),
        ),
    )
    .await;
    let one: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["id"], ada);
    let home: String = sqlx::query_scalar("SELECT personGroupId FROM asset_face WHERE id = ?")
        .bind(&face_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(home, ada);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/faces/{ada}"),
            Some(&cookie),
            Some(&format!(r#"{{"id":"{missing}"}}"#)),
        ),
    )
    .await;
    let no_face: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 400, "{no_face}");
    assert_eq!(no_face["message"], "Not found or no person.create access");

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/faces/{face_id}"),
            Some(&cookie),
            Some(r#"{"force":false}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    assert!(body.is_empty());
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["AssetFacesV3"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let lines = sync_lines(&body);
    let deleted = lines
        .iter()
        .find(|line| line["type"] == "AssetFaceV3" && line["data"]["id"] == face_id)
        .unwrap();
    assert!(deleted["data"]["deletedAt"].as_str().is_some());

    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/faces/{face_id}"),
            Some(&cookie),
            Some(r#"{"force":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&cookie),
            Some(r#"{"types":["AssetFacesV3"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let lines = sync_lines(&body);
    assert!(lines.iter().any(|line| {
        line["type"] == "AssetFaceDeleteV1" && line["data"]["assetFaceId"] == face_id
    }));

    let _ = std::fs::remove_dir_all(&dir);
}
