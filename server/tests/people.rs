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
