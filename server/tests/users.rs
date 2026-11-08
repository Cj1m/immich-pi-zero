use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
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
        machine_learning_enabled: true,
        config_file: None,
        log_level: None,
    }
}

async fn call(app: &App, request: Request<Body>) -> (http::StatusCode, Vec<u8>) {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

fn json_request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .expect("request")
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

async fn login(
    app: &App,
    email: &str,
    password: &str,
    user_agent: &str,
    model: &str,
    device: &str,
) -> (Value, String) {
    let (status, body) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .header("user-agent", user_agent)
            .header("devicemodel", model)
            .header("devicetype", device)
            .body(Body::from(format!(
                r#"{{"email":"{email}","password":"{password}"}}"#
            )))
            .unwrap(),
    )
    .await;
    let body = json_body(&body);
    assert_eq!(status, 201, "{body}");
    let token = body["accessToken"].as_str().expect("token").to_string();
    (body, format!("immich_access_token={token}"))
}

#[tokio::test]
async fn admin_creates_a_later_user() {
    let dir = std::env::temp_dir().join(format!("immich-users-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");

    let (status, body) = call(&app, json_request("GET", "/api/public/config", None, None)).await;
    let public_config = json_body(&body);
    assert_eq!(status, 200, "{public_config}");
    assert_eq!(public_config["passwordLogin"]["enabled"], true);
    assert_eq!(public_config["oauth"]["enabled"], false);
    assert_eq!(public_config["server"]["loginPageMessage"], "");
    assert!(public_config.get("job").is_none());

    let (status, body) = call(&app, json_request("GET", "/api/server/storage", None, None)).await;
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&body));

    let (_, admin_cookie) = login(
        &app,
        "admin@example.com",
        "correct horse",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0 Safari/537.36",
        "Mac",
        "desktop",
    )
    .await;

    let (status, body) = call(
        &app,
        json_request("GET", "/api/server/storage", Some(&admin_cookie), None),
    )
    .await;
    let storage = json_body(&body);
    assert_eq!(status, 200, "{storage}");
    let total = storage["diskSizeRaw"].as_u64().expect("disk size");
    assert!(total > 0, "{storage}");
    assert_eq!(storage["diskSize"], total.to_string());
    assert_eq!(
        storage["diskUse"],
        storage["diskUseRaw"].as_u64().unwrap().to_string()
    );
    assert_eq!(
        storage["diskAvailable"],
        storage["diskAvailableRaw"].as_u64().unwrap().to_string()
    );
    assert!(storage["diskUsagePercentage"].is_number());

    let (status, body) = call(
        &app,
        json_request("POST", "/api/admin/users", Some(&admin_cookie), Some("{}")),
    )
    .await;
    let missing = json_body(&body);
    assert_eq!(status, 400, "{missing}");
    assert_eq!(missing["message"], "password is required");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&admin_cookie),
            Some(
                r#"{"email":"admin@example.com","password":"temp-pass","name":"Again","notify":true}"#,
            ),
        ),
    )
    .await;
    let duplicate = json_body(&body);
    assert_eq!(status, 400, "{duplicate}");
    assert_eq!(duplicate["message"], "Email is not available");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&admin_cookie),
            Some(
                r#"{"email":"  Guest@Example.com ","password":"temp-pass","name":"Guest","notify":true,"quotaSizeInBytes":4096}"#,
            ),
        ),
    )
    .await;
    let created = json_body(&body);
    assert_eq!(status, 201, "{created}");
    let guest_id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["email"], "guest@example.com");
    assert_eq!(created["name"], "Guest");
    assert_eq!(created["profileImagePath"], "");
    assert!(created["deletedAt"].is_null());
    assert_eq!(created["oauthId"], "");
    assert!(created["storageLabel"].is_null());
    assert!(created["license"].is_null());
    assert_eq!(created["status"], "active");
    assert_eq!(created["quotaSizeInBytes"], 4096);
    assert_eq!(created["quotaUsageInBytes"], 0);
    assert_eq!(created["shouldChangePassword"], true);
    assert_eq!(created["isAdmin"], false);
    assert!(created["clusterGroupId"].is_null());
    assert!(created["createdAt"].as_str().unwrap().contains('T'));
    assert!(!created["avatarColor"].as_str().unwrap().is_empty());

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/admin/users?withDeleted=true",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let list = json_body(&body);
    assert_eq!(status, 200, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|user| user["email"] == "guest@example.com" && user["quotaSizeInBytes"] == 4096));

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/admin/users?withDeleted=maybe",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&body));

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users?id={guest_id}&withDeleted=true"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let found = json_body(&body);
    assert_eq!(status, 200, "{found}");
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["id"], guest_id);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/preferences"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let preferences = json_body(&body);
    assert_eq!(status, 200, "{preferences}");
    assert_eq!(preferences["emailNotifications"]["enabled"], true);
    assert_eq!(preferences["folders"]["enabled"], false);
    assert_eq!(preferences["memories"]["enabled"], true);
    assert_eq!(preferences["people"]["enabled"], true);
    assert_eq!(preferences["ratings"]["enabled"], false);
    assert_eq!(preferences["sharedLinks"]["enabled"], true);
    assert_eq!(preferences["purchase"]["showSupportBadge"], true);
    assert_eq!(preferences["tags"]["enabled"], false);
    assert_eq!(preferences["cast"]["gCastEnabled"], false);

    insert_assets(&app, &guest_id).await;
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/statistics"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let stats = json_body(&body);
    assert_eq!(status, 200, "{stats}");
    assert_eq!(stats["images"], 1);
    assert_eq!(stats["videos"], 0);
    assert_eq!(stats["total"], 1);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!(
                "/api/admin/users/{guest_id}/calendar-heatmap?from=2020-01-01&to=2020-01-03&type=Upload"
            ),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let heatmap = json_body(&body);
    assert_eq!(status, 200, "{heatmap}");
    assert_eq!(heatmap["from"], "2020-01-01");
    assert_eq!(heatmap["to"], "2020-01-03");
    assert_eq!(heatmap["series"].as_array().unwrap().len(), 3);
    assert_eq!(heatmap["series"][0]["count"], 1);
    assert_eq!(heatmap["series"][1]["date"], "2020-01-02");
    assert_eq!(heatmap["series"][1]["count"], 1);
    assert_eq!(heatmap["series"][2]["count"], 0);
    assert_eq!(heatmap["totalCount"], 2);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            "/api/admin/users/00000000-0000-4000-8000-000000000000/calendar-heatmap?from=2020-01-01&to=2020-01-01",
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let missing_user = json_body(&body);
    assert_eq!(status, 400, "{missing_user}");
    assert_eq!(missing_user["message"], "User not found");

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 0);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/users/{guest_id}/profile-image"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let image = json_body(&body);
    assert_eq!(status, 404, "{image}");
    assert_eq!(image["message"], "Not found");

    let (login_body, guest_cookie) = login(
        &app,
        "guest@example.com",
        "temp-pass",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0 Safari/537.36",
        "Mac",
        "desktop",
    )
    .await;
    assert_eq!(login_body["shouldChangePassword"], true);
    assert_eq!(login_body["isOnboarded"], false);
    assert_eq!(login_body["isAdmin"], false);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(&guest_cookie),
            Some(r#"{"email":"other@example.com","password":"temp-pass","name":"Other"}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me",
            Some(&guest_cookie),
            Some(r#"{"password":""}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let (status, body) = call(
        &app,
        json_request("GET", "/api/users/me", Some(&guest_cookie), None),
    )
    .await;
    let me = json_body(&body);
    assert_eq!(status, 200, "{me}");
    assert_eq!(me["shouldChangePassword"], true);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me",
            Some(&guest_cookie),
            Some(r#"{"password":"abc"}"#),
        ),
    )
    .await;
    let changed = json_body(&body);
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["shouldChangePassword"], false);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 200);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me/preferences",
            Some(&guest_cookie),
            Some(r#"{"cast":{"gCastEnabled":true}}"#),
        ),
    )
    .await;
    let merged = json_body(&body);
    assert_eq!(status, 200, "{merged}");
    assert_eq!(merged["cast"]["gCastEnabled"], true);
    assert_eq!(merged["memories"]["enabled"], true);
    assert_eq!(merged["people"]["minimumFaces"], 3);

    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me/onboarding",
            Some(&guest_cookie),
            Some(r#"{"isOnboarded":true}"#),
        ),
    )
    .await;
    let onboarded = json_body(&body);
    assert_eq!(status, 200, "{onboarded}");
    assert_eq!(onboarded["isOnboarded"], true);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/system-metadata/admin-onboarding",
            Some(&guest_cookie),
            Some(r#"{"isOnboarded":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));

    let (second_login, ios_cookie) = login(
        &app,
        "guest@example.com",
        "abc",
        "immich-ios/2.0.0",
        "iPhone",
        "iOS",
    )
    .await;
    assert_eq!(second_login["shouldChangePassword"], false);
    assert_eq!(second_login["isOnboarded"], true);

    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 2);
    assert!(sessions.as_array().unwrap().iter().all(|row| {
        row["current"] == false
            && row["isPendingSyncReset"] == false
            && row.get("expiresAt").is_none()
            && row["deviceOS"].is_string()
            && row["deviceType"].is_string()
    }));
    let ios = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["deviceType"] == "iPhone")
        .expect("ios session");
    assert_eq!(ios["deviceOS"], "iOS");
    assert_eq!(ios["appVersion"], "2.0.0");
    let ios_id = ios["id"].as_str().unwrap().to_string();

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/change-password",
            Some(&ios_cookie),
            Some(r#"{"password":"nope","newPassword":"long-enough","invalidateSessions":false}"#),
        ),
    )
    .await;
    let wrong = json_body(&body);
    assert_eq!(status, 400, "{wrong}");
    assert_eq!(wrong["message"], "Wrong password");

    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/change-password",
            Some(&ios_cookie),
            Some(r#"{"password":"abc","newPassword":"short"}"#),
        ),
    )
    .await;
    assert_eq!(status, 400);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/auth/change-password",
            Some(&ios_cookie),
            Some(r#"{"password":"abc","newPassword":"long-enough","invalidateSessions":false}"#),
        ),
    )
    .await;
    let updated = json_body(&body);
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["shouldChangePassword"], false);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&guest_cookie), None),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = call(
        &app,
        json_request("POST", "/api/auth/validateToken", Some(&ios_cookie), None),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/users/{guest_id}/sessions"),
            Some(&admin_cookie),
            None,
        ),
    )
    .await;
    let sessions = json_body(&body);
    assert_eq!(status, 200, "{sessions}");
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["id"], ios_id);

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/albums",
            Some(&admin_cookie),
            Some(r#"{"albumName":"Friends"}"#),
        ),
    )
    .await;
    let album = json_body(&body);
    assert_eq!(status, 201, "{album}");
    let album_id = album["id"].as_str().unwrap();
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/users"),
            Some(&admin_cookie),
            Some(&format!(r#"{{"albumUsers":[{{"userId":"{guest_id}"}}]}}"#)),
        ),
    )
    .await;
    let shared = json_body(&body);
    assert_eq!(status, 200, "{shared}");
    assert!(shared["albumUsers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| { row["role"] == "editor" && row["user"]["id"] == guest_id }));
    let (status, body) = call(
        &app,
        json_request(
            "PUT",
            &format!("/api/albums/{album_id}/users"),
            Some(&admin_cookie),
            Some(r#"{"albumUsers":[{"userId":"00000000-0000-4000-8000-000000000000"}]}"#),
        ),
    )
    .await;
    let invalid = json_body(&body);
    assert_eq!(status, 400, "{invalid}");
    assert_eq!(invalid["message"], "Invalid user");

    let (status, body) = call(
        &app,
        json_request(
            "POST",
            "/api/sync/stream",
            Some(&admin_cookie),
            Some(r#"{"types":["UsersV1"]}"#),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let lines: Vec<Value> = String::from_utf8(body)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let users: Vec<&Value> = lines
        .iter()
        .filter(|line| line["type"] == "UserV1")
        .collect();
    assert!(users
        .iter()
        .any(|line| line["data"]["email"] == "admin@example.com"));
    let guest = users
        .iter()
        .find(|line| line["data"]["email"] == "guest@example.com")
        .expect("guest sync row");
    assert_eq!(guest["data"]["id"], guest_id);
    assert_eq!(guest["data"]["name"], "Guest");
    assert_eq!(guest["data"]["hasProfileImage"], false);
    assert!(guest["data"]["deletedAt"].is_null());
    assert!(lines.iter().any(|line| line["type"] == "SyncCompleteV1"));

    let (status, _) = call(
        &app,
        json_request(
            "POST",
            "/api/system-metadata/admin-onboarding",
            Some(&admin_cookie),
            Some(r#"{"isOnboarded":true}"#),
        ),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body) = call(&app, json_request("GET", "/api/server/config", None, None)).await;
    let server = json_body(&body);
    assert_eq!(status, 200, "{server}");
    assert_eq!(server["isOnboarded"], true);

    let _ = std::fs::remove_dir_all(&dir);
}

async fn insert_assets(app: &App, owner_id: &str) {
    for (created_at, visibility, deleted_at, mark) in [
        ("2020-01-02T15:00:00.000Z", "timeline", None, 1_u8),
        ("2020-01-01T15:00:00.000Z", "hidden", None, 2),
        (
            "2020-01-03T15:00:00.000Z",
            "timeline",
            Some("2020-01-04T00:00:00.000Z"),
            3,
        ),
    ] {
        sqlx::query(
            "INSERT INTO asset (
                id, ownerId, type, originalPath, originalFileName, originalMimeType,
                fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
                deletedAt, createdAt, updatedAt, updateId
            ) VALUES (?, ?, 'IMAGE', 'object', 'a.jpg', 'image/jpeg', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(owner_id)
        .bind(created_at)
        .bind(created_at)
        .bind(created_at)
        .bind(vec![mark])
        .bind(visibility)
        .bind(deleted_at)
        .bind(created_at)
        .bind(created_at)
        .bind(Uuid::now_v7().to_string())
        .execute(app.pool())
        .await
        .expect("asset");
    }
}
