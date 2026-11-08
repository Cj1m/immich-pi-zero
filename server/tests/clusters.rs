//! Cluster groups: invite, accept, leave, and the person split. Nothing here creates the first group.

use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use immich_server::{App, Config};
use serde_json::Value;
use tower::ServiceExt;

const GROUP: &str = "11111111-1111-4111-8111-111111111111";
const UNKNOWN: &str = "99999999-9999-4999-8999-999999999999";

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

async fn sign_in(app: &App, email: &str, password: &str) -> String {
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

async fn create_user(app: &App, admin_cookie: &str, email: &str, name: &str) -> (String, String) {
    let body =
        format!(r#"{{"email":"{email}","password":"temp-pass","name":"{name}","notify":false}}"#);
    let (status, bytes) = call(
        app,
        json_request("POST", "/api/admin/users", Some(admin_cookie), Some(&body)),
    )
    .await;
    let created = json_body(&bytes);
    assert_eq!(status, 201, "{created}");
    assert!(created["clusterGroupId"].is_null(), "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let cookie = sign_in(app, email, "temp-pass").await;
    (id, cookie)
}

async fn count(app: &App, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(app.pool()).await.unwrap()
}

async fn insert_asset(app: &App, owner: &str, mark: u8) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO asset (
            id, ownerId, type, originalPath, originalFileName, originalMimeType,
            fileCreatedAt, fileModifiedAt, localDateTime, checksum, updateId, createdAt, updatedAt
         ) VALUES (?, ?, 'IMAGE', 'mem', 'photo.jpg', 'image/jpeg', ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(vec![mark])
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(now)
    .bind(now)
    .execute(app.pool())
    .await
    .unwrap();
    id
}

async fn insert_group(app: &App, cluster: Option<&str>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    let update_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(cluster)
    .bind(now)
    .bind(&update_id)
    .bind(now)
    .bind(&update_id)
    .execute(app.pool())
    .await
    .unwrap();
    id
}

async fn insert_person(app: &App, owner: &str, group: &str, name: &str) -> String {
    let now = "2020-01-02T03:04:05.000Z";
    let update_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO person (
            ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden,
            birthDate, faceAssetId, isFavorite, color, updateId
         ) VALUES (?, ?, ?, ?, ?, '', 0, NULL, NULL, 0, NULL, ?)",
    )
    .bind(owner)
    .bind(group)
    .bind(now)
    .bind(now)
    .bind(name)
    .bind(&update_id)
    .execute(app.pool())
    .await
    .unwrap();
    update_id
}

async fn insert_face(app: &App, owner: &str, asset: &str, group: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO asset_face (
            id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
            boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, isVisible,
            ownerId, assetVisibility, assetDeletedAt
         ) VALUES (?, ?, ?, 16, 10, 0, 0, 4, 4, 'machine-learning', NULL, ?, ?, 1, ?, 'timeline', NULL)",
    )
    .bind(&id)
    .bind(asset)
    .bind(group)
    .bind(now)
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(owner)
    .execute(app.pool())
    .await
    .unwrap();
    id
}

fn ids_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn cluster_groups_join_leave_and_split() {
    let dir = std::env::temp_dir().join(format!("immich-cluster-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let admin_cookie = sign_in(&app, "admin@example.com", "correct horse").await;
    let (status, body) = call(
        &app,
        json_request("GET", "/api/users/me", Some(&admin_cookie), None),
    )
    .await;
    let me = json_body(&body);
    assert_eq!(status, 200, "{me}");
    assert!(me["clusterGroupId"].is_null(), "{me}");
    let admin_id = me["id"].as_str().unwrap().to_string();

    let (guest_id, guest_cookie) =
        create_user(&app, &admin_cookie, "guest@example.com", "Guest").await;
    let (bea_id, _) = create_user(&app, &admin_cookie, "bea@example.com", "Bea").await;
    let (zoe_id, _) = create_user(&app, &admin_cookie, "zoe@example.com", "Zoe").await;
    let (eve_id, eve_cookie) = create_user(&app, &admin_cookie, "eve@example.com", "Eve").await;
    let (cara_id, cara_cookie) = create_user(&app, &admin_cookie, "cara@example.com", "Cara").await;
    let (gone_id, _) = create_user(&app, &admin_cookie, "gone@example.com", "Gone").await;
    assert_eq!(count(&app, "SELECT COUNT(*) FROM cluster_group").await, 0);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/people", Some(&admin_cookie), None),
    )
    .await;
    assert_eq!(status, 200, "{}", json_body(&body));
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/null/users",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "Validation failed");
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/not-a-uuid/requests",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");

    let (status, body) = call(
        &app,
        json_request("GET", "/api/cluster-groups/requests", None, None),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(message(&body), "Authentication required");
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/requests",
            Some("immich_shared_link_token=abc"),
            None,
        ),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(message(&body), "Forbidden");
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/requests?key=abc",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(message(&body), "Invalid share key");

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/requests",
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    let mine = json_body(&body);
    assert_eq!(status, 200, "{mine}");
    assert_eq!(mine, serde_json::json!([]));

    let now = "2020-01-02T03:04:05.000Z";
    let update_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO cluster_group (id, name, createdAt, updatedAt, updateId)
         VALUES (?, NULL, ?, ?, ?)",
    )
    .bind(GROUP)
    .bind(now)
    .bind(now)
    .bind(&update_id)
    .execute(app.pool())
    .await
    .unwrap();
    for id in [&admin_id, &bea_id, &zoe_id] {
        sqlx::query("UPDATE user SET clusterGroupId = ? WHERE id = ?")
            .bind(GROUP)
            .bind(id)
            .execute(app.pool())
            .await
            .unwrap();
    }
    sqlx::query("UPDATE user SET deletedAt = ? WHERE id = ?")
        .bind(now)
        .bind(&gone_id)
        .execute(app.pool())
        .await
        .unwrap();

    let users_path = format!("/api/cluster-groups/{GROUP}/users");
    let requests_path = format!("/api/cluster-groups/{GROUP}/requests");
    let leave_path = format!("/api/cluster-groups/{GROUP}/leave");
    let regenerate_path = format!("/api/cluster-groups/{GROUP}/regenerate-people");

    for (cookie, path) in [
        (guest_cookie.as_str(), users_path.as_str()),
        (guest_cookie.as_str(), requests_path.as_str()),
        (eve_cookie.as_str(), users_path.as_str()),
    ] {
        let (status, body) = call(&app, json_request("GET", path, Some(cookie), None)).await;
        assert_eq!(status, 400, "{}", json_body(&body));
        assert_eq!(message(&body), "Not found or no clusterGroup.read access");
    }
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&guest_cookie),
            Some(&format!(r#"{{"userId":"{eve_id}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        message(&body),
        "Not found or no clusterGroupRequest.create access"
    );
    let (status, body) = call(
        &app,
        json_request("POST", &leave_path, Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Not found or no clusterGroup.leave access");
    let (status, body) = call(
        &app,
        json_request("POST", &regenerate_path, Some(&eve_cookie), None),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Not found or no clusterGroup.read access");

    let (status, body) = call(
        &app,
        json_request("PUT", &requests_path, Some(&admin_cookie), Some("{}")),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");
    let (status, body) = call(
        &app,
        json_request("PUT", &requests_path, Some(&admin_cookie), Some("[]")),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/cluster-groups/null/requests",
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{guest_id}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(r#"{"userId":"nope"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{admin_id}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        message(&body),
        "Cannot request to join your own cluster group"
    );
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{UNKNOWN}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "User not found");
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{gone_id}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "User not found");

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{guest_id}"}}"#)),
        ),
    )
    .await;
    let invited = json_body(&body);
    assert_eq!(status, 201, "{invited}");
    assert_eq!(invited["clusterGroupId"], GROUP);
    assert_eq!(invited["userId"], guest_id);
    let guest_request = invited["id"].as_str().unwrap().to_string();
    let invited_at = invited["createdAt"].as_str().unwrap().to_string();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{guest_id}"}}"#)),
        ),
    )
    .await;
    let again = json_body(&body);
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["id"], guest_request);
    assert_eq!(again["createdAt"], invited_at);

    let (status, body) = call(
        &app,
        json_request("GET", "/api/notifications", Some(&guest_cookie), None),
    )
    .await;
    let notes = json_body(&body);
    assert_eq!(status, 200, "{notes}");
    assert_eq!(notes.as_array().unwrap().len(), 1);
    assert_eq!(notes[0]["type"], "ClusterGroupRequest");
    assert_eq!(notes[0]["level"], "info");
    assert_eq!(notes[0]["title"], "Cluster Group Request");
    assert_eq!(
        notes[0]["description"],
        "Ada asked you to join their cluster group"
    );
    assert_eq!(notes[0]["data"]["clusterGroupId"], GROUP);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{cara_id}"}}"#)),
        ),
    )
    .await;
    let cara_invite = json_body(&body);
    assert_eq!(status, 201, "{cara_invite}");
    let cara_request = cara_invite["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE cluster_group_request SET createdAt = ? WHERE id = ?")
        .bind("2020-01-01T00:00:00.000Z")
        .bind(&guest_request)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE cluster_group_request SET createdAt = ? WHERE id = ?")
        .bind("2021-01-01T00:00:00.000Z")
        .bind(&cara_request)
        .execute(app.pool())
        .await
        .unwrap();

    let (status, body) = call(
        &app,
        json_request("GET", &requests_path, Some(&admin_cookie), None),
    )
    .await;
    let outgoing = json_body(&body);
    assert_eq!(status, 200, "{outgoing}");
    assert_eq!(
        ids_of(&outgoing),
        vec![guest_request.clone(), cara_request.clone()]
    );
    let (status, body) = call(
        &app,
        json_request("GET", &requests_path, Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(message(&body), "Not found or no clusterGroup.read access");
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/requests",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(json_body(&body), serde_json::json!([]));
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/requests",
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    let guest_requests = json_body(&body);
    assert_eq!(status, 200, "{guest_requests}");
    assert_eq!(ids_of(&guest_requests), vec![guest_request.clone()]);

    let (status, body) = call(
        &app,
        json_request("GET", &users_path, Some(&guest_cookie), None),
    )
    .await;
    let users = json_body(&body);
    assert_eq!(status, 200, "{users}");
    assert_eq!(
        ids_of(&users),
        vec![admin_id.clone(), bea_id.clone(), zoe_id.clone()]
    );
    assert!(users[0].get("clusterGroupId").is_none(), "{users}");
    assert_eq!(users[0]["name"], "Ada");
    assert!(!users[0]["avatarColor"].as_str().unwrap().is_empty());

    let (status, body) = call(
        &app,
        json_request("POST", &regenerate_path, Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));

    let old_cluster = "22222222-2222-4222-8222-222222222222";
    let exclusive = insert_group(&app, None).await;
    let shared = insert_group(&app, Some(old_cluster)).await;
    let exclusive_update = insert_person(&app, &guest_id, &exclusive, "Only").await;
    insert_person(&app, &admin_id, &shared, "Shared").await;
    let shared_person_update = insert_person(&app, &guest_id, &shared, "Shared").await;
    let guest_asset = insert_asset(&app, &guest_id, 1).await;
    let admin_asset = insert_asset(&app, &admin_id, 2).await;
    let exclusive_face = insert_face(&app, &guest_id, &guest_asset, &exclusive).await;
    let shared_face = insert_face(&app, &guest_id, &guest_asset, &shared).await;
    let admin_face = insert_face(&app, &admin_id, &admin_asset, &shared).await;
    let faces_before = count(&app, "SELECT COUNT(*) FROM asset_face").await;
    let guest_user_update: String = sqlx::query_scalar("SELECT updateId FROM user WHERE id = ?")
        .bind(&guest_id)
        .fetch_one(app.pool())
        .await
        .unwrap();

    let (status, body) = call(
        &app,
        json_request("POST", &regenerate_path, Some(&admin_cookie), None),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM asset_face").await,
        faces_before
    );
    assert_eq!(count(&app, "SELECT COUNT(*) FROM person_audit").await, 0);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/cluster-groups/requests/{UNKNOWN}/accept"),
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        message(&body),
        "Not found or no clusterGroupRequest.read access"
    );
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/cluster-groups/requests/{guest_request}/accept"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        message(&body),
        "Not found or no clusterGroupRequest.read access"
    );
    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/cluster-groups/requests/not-a-uuid/accept",
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Validation failed");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/cluster-groups/requests/{guest_request}/accept"),
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
    assert_eq!(count(&app, "SELECT COUNT(*) FROM cluster_group").await, 1);
    let (status, body) = call(
        &app,
        json_request("GET", "/api/users/me", Some(&guest_cookie), None),
    )
    .await;
    let guest_me = json_body(&body);
    assert_eq!(status, 200, "{guest_me}");
    assert_eq!(guest_me["clusterGroupId"], GROUP);
    let guest_user_update_after: String =
        sqlx::query_scalar("SELECT updateId FROM user WHERE id = ?")
            .bind(&guest_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_ne!(guest_user_update_after, guest_user_update);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/cluster-groups/requests",
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json_body(&body), serde_json::json!([]));

    let exclusive_cluster: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&exclusive)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(exclusive_cluster.as_deref(), Some(GROUP));
    let exclusive_person_update: String =
        sqlx::query_scalar("SELECT updateId FROM person WHERE ownerId = ? AND personGroupId = ?")
            .bind(&guest_id)
            .bind(&exclusive)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(exclusive_person_update, exclusive_update);
    let shared_cluster: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&shared)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(shared_cluster.as_deref(), Some(old_cluster));
    let admin_still: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM person WHERE ownerId = ? AND personGroupId = ? AND name = 'Shared'",
    )
    .bind(&admin_id)
    .bind(&shared)
    .fetch_one(app.pool())
    .await
    .unwrap();
    assert_eq!(admin_still, 1);
    let (moved_group, moved_update): (String, String) = sqlx::query_as(
        "SELECT personGroupId, updateId FROM person WHERE ownerId = ? AND name = 'Shared'",
    )
    .bind(&guest_id)
    .fetch_one(app.pool())
    .await
    .unwrap();
    assert_ne!(moved_group, shared);
    assert_ne!(moved_update, shared_person_update);
    let moved_cluster: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&moved_group)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(moved_cluster.as_deref(), Some(GROUP));
    let exclusive_face_group: Option<String> =
        sqlx::query_scalar("SELECT personGroupId FROM asset_face WHERE id = ?")
            .bind(&exclusive_face)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(exclusive_face_group.as_deref(), Some(exclusive.as_str()));
    let shared_face_group: Option<String> =
        sqlx::query_scalar("SELECT personGroupId FROM asset_face WHERE id = ?")
            .bind(&shared_face)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(shared_face_group.as_deref(), Some(moved_group.as_str()));
    let admin_face_group: Option<String> =
        sqlx::query_scalar("SELECT personGroupId FROM asset_face WHERE id = ?")
            .bind(&admin_face)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(admin_face_group.as_deref(), Some(shared.as_str()));
    assert_eq!(count(&app, "SELECT COUNT(*) FROM person_audit").await, 0);
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM asset_face").await,
        faces_before
    );

    let (status, body) = call(
        &app,
        json_request("GET", &users_path, Some(&guest_cookie), None),
    )
    .await;
    let users = json_body(&body);
    assert_eq!(status, 200, "{users}");
    assert_eq!(
        ids_of(&users),
        vec![
            guest_id.clone(),
            admin_id.clone(),
            bea_id.clone(),
            zoe_id.clone()
        ]
    );

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            &format!("/api/cluster-groups/requests/{guest_request}/accept"),
            Some(&guest_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        message(&body),
        "Not found or no clusterGroupRequest.read access"
    );

    let (status, _) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/cluster-groups/requests/{cara_request}"),
            Some(&cara_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{cara_id}"}}"#)),
        ),
    )
    .await;
    let cara_again = json_body(&body);
    assert_eq!(status, 201, "{cara_again}");
    let cara_request = cara_again["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/cluster-groups/requests/{cara_request}"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &requests_path,
            Some(&admin_cookie),
            Some(&format!(r#"{{"userId":"{cara_id}"}}"#)),
        ),
    )
    .await;
    let cara_third = json_body(&body);
    assert_eq!(status, 201, "{cara_third}");
    let cara_request = cara_third["id"].as_str().unwrap().to_string();
    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/cluster-groups/requests/{cara_request}"),
            Some(&eve_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        message(&body),
        "Not found or no clusterGroupRequest.delete access"
    );
    let (status, body) = call(
        &app,
        json_request(
            "DELETE",
            &format!("/api/cluster-groups/requests/{UNKNOWN}"),
            Some(&cara_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        message(&body),
        "Not found or no clusterGroupRequest.delete access"
    );
    let still_there: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cluster_group_request WHERE id = ?")
            .bind(&cara_request)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(still_there, 1);

    sqlx::query("UPDATE user SET deletedAt = ? WHERE id = ?")
        .bind(now)
        .bind(&zoe_id)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, body) = call(
        &app,
        json_request("GET", &users_path, Some(&admin_cookie), None),
    )
    .await;
    let users = json_body(&body);
    assert_eq!(status, 200, "{users}");
    assert!(!ids_of(&users).contains(&zoe_id));
    let (status, _) = call(
        &app,
        json_request("POST", &leave_path, Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM cluster_group").await, 2);
    let guest_cluster: String = sqlx::query_scalar("SELECT clusterGroupId FROM user WHERE id = ?")
        .bind(&guest_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_ne!(guest_cluster, GROUP);
    let guest_group_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM cluster_group WHERE id = ?")
            .bind(&guest_cluster)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert!(guest_group_name.is_none());
    let exclusive_after: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&exclusive)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(exclusive_after.as_deref(), Some(guest_cluster.as_str()));
    let moved_after: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&moved_group)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(moved_after.as_deref(), Some(guest_cluster.as_str()));
    let guest_person_still: String = sqlx::query_scalar(
        "SELECT personGroupId FROM person WHERE ownerId = ? AND name = 'Shared'",
    )
    .bind(&guest_id)
    .fetch_one(app.pool())
    .await
    .unwrap();
    assert_eq!(guest_person_still, moved_group);
    let shared_stays: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&shared)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(shared_stays.as_deref(), Some(old_cluster));

    sqlx::query("UPDATE user SET deletedAt = ? WHERE id = ?")
        .bind(now)
        .bind(&bea_id)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, body) = call(
        &app,
        json_request("POST", &leave_path, Some(&admin_cookie), None),
    )
    .await;
    assert_eq!(status, 400, "{}", json_body(&body));
    assert_eq!(
        message(&body),
        "Cannot leave a cluster group without any other members"
    );
    sqlx::query("UPDATE user SET deletedAt = NULL WHERE id = ?")
        .bind(&bea_id)
        .execute(app.pool())
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        json_request("POST", &leave_path, Some(&admin_cookie), None),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM cluster_group").await, 3);
    let admin_cluster: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM user WHERE id = ?")
            .bind(&admin_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    let admin_cluster = admin_cluster.unwrap();
    assert_ne!(admin_cluster, GROUP);
    let admin_group_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM cluster_group WHERE id = ?")
            .bind(&admin_cluster)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert!(admin_group_name.is_none());
    let shared_moved: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM person_group WHERE id = ?")
            .bind(&shared)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(shared_moved.as_deref(), Some(admin_cluster.as_str()));
    let admin_person: String = sqlx::query_scalar(
        "SELECT personGroupId FROM person WHERE ownerId = ? AND name = 'Shared'",
    )
    .bind(&admin_id)
    .fetch_one(app.pool())
    .await
    .unwrap();
    assert_eq!(admin_person, shared);
    let (status, body) = call(
        &app,
        json_request("GET", "/api/users/me", Some(&admin_cookie), None),
    )
    .await;
    let admin_me = json_body(&body);
    assert_eq!(status, 200, "{admin_me}");
    assert_eq!(admin_me["clusterGroupId"], admin_cluster);
    let (status, body) = call(
        &app,
        json_request("POST", &leave_path, Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(message(&body), "Not found or no clusterGroup.leave access");
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM asset_face").await,
        faces_before
    );
    assert_eq!(count(&app, "SELECT COUNT(*) FROM person_audit").await, 0);

    let _ = std::fs::remove_dir_all(&dir);
}
