//! Admin queues, manual jobs, maintenance, backups, integrity, license, and statistics.

use std::net::IpAddr;
use std::path::Path;

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use immich_server::{App, Config};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tower::ServiceExt;
use uuid::Uuid;

const SERVER_LICENSE: &str = "IMSV-6ECZ-91TE-WZRM-Q7AQ-MBN4-UW48-2CPT-71X9";
const SERVER_ACTIVATION: &str = "4kJUNUWMq13J14zqPFm1NodRcI6MV6DeOGvQNIgrM8Sc9nv669wyEVvFw1Nz4Kb1W7zLWblOtXEQzpRRqC4r4fKjewJxfbpeo9sEsqAVIfl4Ero-Vp1Dg21-sVdDGZEAy2oeTCXAyCT5d1JqrqR6N1qTAm4xOx9ujXQRFYhjRG8uwudw7_Q49pF18Tj5OEv9qCqElxztoNck4i6O_azsmsoOQrLIENIWPh3EynBN3ESpYERdCgXO8MlWeuG14_V1HbNjnJPZDuvYg__YfMzoOEtfm1sCqEaJ2Ww-BaX7yGfuCL4XsuZlCQQNHjfscy_WywVfIZPKCiW8QR74i0cSzQ";
const USER_LICENSE: &str = "IMCL-FF69-TUK1-RWZU-V9Q8-QGQS-S5GC-X4R2-UFK4";
const USER_ACTIVATION: &str = "KuX8KsktrBSiXpQMAH0zLgA5SpijXVr_PDkzLdWUlAogCTMBZ0I3KCHXK0eE9EEd7harxup8_EHMeqAWeHo5VQzol6LGECpFv585U9asXD4Zc-UXt3mhJr2uhazqipBIBwJA2YhmUCDy8hiyiGsukDQNu9Rg9C77UeoKuZBWVjWUBWG0mc1iRqfvF0faVM20w53czAzlhaMxzVGc3Oimbd7xi_CAMSujF_2y8QpA3X2fOVkQkzdcH9lV0COejl7IyH27zQQ9HrlrXv3Lai5Hw67kNkaSjmunVBxC5PS0TpKoc9SfBJMaAGWnaDbjhjYUrm-8nIDQnoeEAidDXVAdPw";

const QUEUES: [&str; 19] = [
    "thumbnailGeneration",
    "metadataExtraction",
    "videoConversion",
    "faceDetection",
    "facialRecognition",
    "smartSearch",
    "duplicateDetection",
    "backgroundTask",
    "storageTemplateMigration",
    "migration",
    "search",
    "sidecar",
    "library",
    "notifications",
    "backupDatabase",
    "ocr",
    "workflow",
    "integrityCheck",
    "editor",
];

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

struct Reply {
    status: http::StatusCode,
    body: Vec<u8>,
    cookies: Vec<String>,
    content_type: Option<String>,
}

async fn call(app: &App, request: Request<Body>) -> Reply {
    let response = immich_server::router(app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_string))
        .collect();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Reply {
        status,
        body,
        cookies,
        content_type,
    }
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

async fn create_user(app: &App, admin: &str, email: &str, name: &str) -> String {
    let reply = call(
        app,
        json_request(
            "POST",
            "/api/admin/users",
            Some(admin),
            Some(&format!(
                r#"{{"email":"{email}","password":"temp-pass","name":"{name}"}}"#
            )),
        ),
    )
    .await;
    let created = json_body(&reply.body);
    assert_eq!(reply.status, 201, "{created}");
    created["id"].as_str().unwrap().to_string()
}

async fn count(app: &App, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(app.pool()).await.unwrap()
}

fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha1::digest(bytes).to_vec()
}

struct AssetSeed<'a> {
    kind: &'a str,
    visibility: &'a str,
    deleted_at: Option<&'a str>,
    size: i64,
    bytes: &'a [u8],
    checksum: &'a [u8],
    write_file: bool,
}

async fn insert_asset(app: &App, dir: &Path, owner: &str, seed: AssetSeed<'_>) -> String {
    let id = Uuid::new_v4().to_string();
    let key = format!("{owner}/{id}/original");
    if seed.write_file {
        let object = dir.join("objects").join(&key);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(object, seed.bytes).unwrap();
    }
    let now = "2020-01-02T03:04:05.000Z";
    sqlx::query(
        "INSERT INTO asset (
            id, ownerId, type, originalPath, originalFileName, originalMimeType,
            fileCreatedAt, fileModifiedAt, localDateTime, checksum, visibility,
            deletedAt, updateId, createdAt, updatedAt
         ) VALUES (?, ?, ?, ?, 'photo.jpg', 'image/jpeg', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner)
    .bind(seed.kind)
    .bind(&key)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(seed.checksum)
    .bind(seed.visibility)
    .bind(seed.deleted_at)
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
    .bind(seed.size)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    id
}

fn cookie_value(cookies: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    cookies.iter().find_map(|cookie| {
        let pair = cookie.split(';').next()?;
        pair.strip_prefix(&prefix).map(str::to_string)
    })
}

#[tokio::test]
async fn admin_queues_maintenance_and_license() {
    // The published fixtures are signed with the staging keys. Unset IMMICH_ENV is production.
    std::env::set_var("IMMICH_ENV", "testing");
    let dir = std::env::temp_dir().join(format!("immich-admin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::prepare(&config(&dir)).await.expect("prepare");
    let admin = sign_in(&app, "admin@example.com", "correct horse").await;
    let me = call(
        &app,
        json_request("GET", "/api/users/me", Some(&admin), None),
    )
    .await;
    let me_body = json_body(&me.body);
    assert_eq!(me.status, 200, "{me_body}");
    let admin_id = me_body["id"].as_str().unwrap().to_string();
    let guest_id = create_user(&app, &admin, "bea@example.com", "Bea").await;
    let guest = sign_in(&app, "bea@example.com", "temp-pass").await;

    let reply = call(&app, json_request("GET", "/api/queues", None, None)).await;
    assert_eq!(reply.status, 401);
    assert_eq!(message(&reply.body), "Authentication required");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/queues",
            Some("immich_shared_link_token=abc"),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 401);
    assert_eq!(message(&reply.body), "Authentication required");
    let reply = call(&app, json_request("GET", "/api/queues", Some(&guest), None)).await;
    assert_eq!(reply.status, 403);
    assert_eq!(message(&reply.body), "Forbidden");
    let reply = call(
        &app,
        json_request("GET", "/api/queues?key=abc", Some(&admin), None),
    )
    .await;
    let queues = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{queues}");
    let names: Vec<_> = queues
        .as_array()
        .unwrap()
        .iter()
        .map(|queue| queue["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, QUEUES);
    assert!(queues.as_array().unwrap().iter().all(|queue| {
        !queue["isPaused"].as_bool().unwrap()
            && queue["statistics"]["active"].as_i64() == Some(0)
            && queue["statistics"]["completed"].as_i64() == Some(0)
    }));

    let reply = call(
        &app,
        json_request("GET", "/api/queues/not-a-queue", Some(&admin), None),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/queues/backgroundTask",
            Some(&admin),
            Some(r#"{"isPaused":true}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        message(&reply.body),
        "The BackgroundTask queue cannot be paused"
    );
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/jobs/backgroundTask",
            Some(&admin),
            Some(r#"{"command":"pause"}"#),
        ),
    )
    .await;
    let legacy = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{legacy}");
    assert_eq!(legacy["queueStatus"]["isPaused"], true);
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/jobs/backgroundTask",
            Some(&admin),
            Some(r#"{"command":"resume"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 200);

    sqlx::query(
        "INSERT INTO queue_state (name, isPaused, completed) VALUES ('thumbnailGeneration', 0, 4)",
    )
    .execute(app.pool())
    .await
    .unwrap();
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/queues/thumbnailGeneration",
            Some(&admin),
            Some(r#"{"isPaused":true}"#),
        ),
    )
    .await;
    let paused = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{paused}");
    assert_eq!(paused["isPaused"], true);
    assert_eq!(paused["statistics"]["completed"], 4);
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/queues/thumbnailGeneration",
            Some(&admin),
            Some(r#"{"isPaused":false}"#),
        ),
    )
    .await;
    let resumed = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{resumed}");
    assert_eq!(resumed["isPaused"], false);
    assert_eq!(resumed["statistics"]["completed"], 4);
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/queues/thumbnailGeneration",
            Some(&admin),
            Some("[]"),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");

    for (id, status, timestamp) in [
        ("job-wait", "waiting", 30_i64),
        ("job-fail", "failed", 20),
        ("job-run", "active", 10),
    ] {
        sqlx::query(
            "INSERT INTO queue_job (id, queueName, name, data, timestamp, status)
             VALUES (?, 'thumbnailGeneration', 'thumb', '{\"source\":\"test\"}', ?, ?)",
        )
        .bind(id)
        .bind(timestamp)
        .bind(status)
        .execute(app.pool())
        .await
        .unwrap();
    }
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/queues/thumbnailGeneration/jobs",
            Some(&admin),
            None,
        ),
    )
    .await;
    let jobs = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{jobs}");
    let ids: Vec<_> = jobs
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["job-wait", "job-fail", "job-run"]);
    assert_eq!(jobs[0]["data"]["source"], "test");
    assert_eq!(jobs[0]["timestamp"], 30);
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/queues/thumbnailGeneration/jobs?status=failed&status=nope",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/queues/thumbnailGeneration/jobs?status=failed&status=active",
            Some(&admin),
            None,
        ),
    )
    .await;
    let filtered = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{filtered}");
    assert_eq!(filtered.as_array().unwrap().len(), 2);
    let reply = call(
        &app,
        json_request(
            "DELETE",
            "/api/queues/thumbnailGeneration/jobs",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 204);
    assert!(reply.body.is_empty());
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/queues/thumbnailGeneration/jobs",
            Some(&admin),
            None,
        ),
    )
    .await;
    let left = json_body(&reply.body);
    let left_ids: Vec<_> = left
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["id"].as_str().unwrap())
        .collect();
    assert_eq!(left_ids, ["job-fail", "job-run"]);
    let reply = call(
        &app,
        json_request(
            "DELETE",
            "/api/queues/thumbnailGeneration/jobs",
            Some(&admin),
            Some(r#"{"failed":true}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 204);
    let reply = call(
        &app,
        json_request("GET", "/api/queues/thumbnailGeneration", Some(&admin), None),
    )
    .await;
    let after_empty = json_body(&reply.body);
    assert_eq!(after_empty["statistics"]["completed"], 4);
    assert_eq!(after_empty["statistics"]["active"], 1);
    assert_eq!(after_empty["statistics"]["failed"], 0);

    sqlx::query(
        "INSERT INTO queue_job (id, queueName, name, data, timestamp, status)
         VALUES ('video-run', 'videoConversion', 'video', '{}', 1, 'active')",
    )
    .execute(app.pool())
    .await
    .unwrap();
    let files_before = count(&app, "SELECT COUNT(*) FROM asset_file").await;
    let assets_before = count(&app, "SELECT COUNT(*) FROM asset").await;
    let faces_before = count(&app, "SELECT COUNT(*) FROM asset_face").await;
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/jobs/videoConversion",
            Some(&admin),
            Some(r#"{"command":"start"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Job is already running");
    sqlx::query("DELETE FROM queue_job WHERE id = 'video-run'")
        .execute(app.pool())
        .await
        .unwrap();
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/jobs/videoConversion",
            Some(&admin),
            Some(r#"{"command":"start","force":true}"#),
        ),
    )
    .await;
    let started = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{started}");
    assert_eq!(started["queueStatus"]["isActive"], false);
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM asset_file").await,
        files_before
    );
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/jobs/backgroundTask",
            Some(&admin),
            Some(r#"{"command":"start"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Invalid job name: backgroundTask");
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/jobs/videoConversion",
            Some(&admin),
            Some(r#"{"command":"start","force":"yes"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/jobs",
            Some(&admin),
            Some(r#"{"name":"nope"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Invalid job name");
    let reply = call(
        &app,
        json_request("POST", "/api/jobs", Some(&admin), Some("{}")),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Invalid job name");
    for name in ["faceDetection", "facialRecognition", "ocr", "library"] {
        let reply = call(
            &app,
            json_request(
                "PUT",
                &format!("/api/jobs/{name}"),
                Some(&admin),
                Some(r#"{"command":"start"}"#),
            ),
        )
        .await;
        assert_eq!(reply.status, 200, "{}", message(&reply.body));
    }
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM asset").await,
        assets_before
    );
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM asset_face").await,
        faces_before
    );
    let reply = call(&app, json_request("GET", "/api/jobs", Some(&admin), None)).await;
    let jobs = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{jobs}");
    assert!(jobs["videoConversion"]["queueStatus"]["isActive"].is_boolean());
    assert!(jobs["integrityCheck"]["queueStatus"].is_object());

    let photo = b"timeline-photo";
    let hidden = b"hidden-photo";
    let video = b"timeline-video";
    let trashed = b"trashed-photo";
    let photo_sum = digest(photo);
    let hidden_sum = digest(hidden);
    let video_sum = digest(video);
    let trashed_sum = digest(trashed);
    insert_asset(
        &app,
        &dir,
        &admin_id,
        AssetSeed {
            kind: "IMAGE",
            visibility: "timeline",
            deleted_at: None,
            size: 100,
            bytes: photo,
            checksum: &photo_sum,
            write_file: true,
        },
    )
    .await;
    insert_asset(
        &app,
        &dir,
        &admin_id,
        AssetSeed {
            kind: "IMAGE",
            visibility: "hidden",
            deleted_at: None,
            size: 10,
            bytes: hidden,
            checksum: &hidden_sum,
            write_file: true,
        },
    )
    .await;
    insert_asset(
        &app,
        &dir,
        &admin_id,
        AssetSeed {
            kind: "VIDEO",
            visibility: "timeline",
            deleted_at: None,
            size: 50,
            bytes: video,
            checksum: &video_sum,
            write_file: true,
        },
    )
    .await;
    insert_asset(
        &app,
        &dir,
        &admin_id,
        AssetSeed {
            kind: "IMAGE",
            visibility: "timeline",
            deleted_at: Some("2020-01-04T00:00:00.000Z"),
            size: 999,
            bytes: trashed,
            checksum: &trashed_sum,
            write_file: true,
        },
    )
    .await;
    let reply = call(
        &app,
        json_request("GET", "/api/server/statistics", Some(&admin), None),
    )
    .await;
    let stats = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{stats}");
    assert_eq!(stats["photos"], 1);
    assert_eq!(stats["videos"], 1);
    assert_eq!(stats["usage"], 160);
    assert_eq!(stats["usagePhotos"], 110);
    assert_eq!(stats["usageVideos"], 50);
    let users = stats["usageByUser"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0]["userName"], "Ada");
    assert_eq!(users[0]["photos"], 1);
    assert_eq!(users[0]["usage"], 160);
    assert!(users[0]["quotaSizeInBytes"].is_null());
    assert_eq!(users[1]["userId"], guest_id);
    assert_eq!(users[1]["userName"], "Bea");
    assert_eq!(users[1]["photos"], 0);
    assert_eq!(users[1]["videos"], 0);
    assert_eq!(users[1]["usage"], 0);
    let reply = call(
        &app,
        json_request("GET", "/api/server/statistics", Some(&guest), None),
    )
    .await;
    assert_eq!(reply.status, 403);

    let bad = b"checksum-bytes";
    let bad_sum = [1_u8, 2, 3, 4];
    let missing_sum = [5_u8, 6, 7, 8];
    let bad_id = insert_asset(
        &app,
        &dir,
        &admin_id,
        AssetSeed {
            kind: "IMAGE",
            visibility: "timeline",
            deleted_at: None,
            size: 1,
            bytes: bad,
            checksum: &bad_sum,
            write_file: true,
        },
    )
    .await;
    let missing_id = insert_asset(
        &app,
        &dir,
        &admin_id,
        AssetSeed {
            kind: "IMAGE",
            visibility: "timeline",
            deleted_at: None,
            size: 1,
            bytes: b"absent",
            checksum: &missing_sum,
            write_file: false,
        },
    )
    .await;
    let thumb_path = format!("thumbs/{bad_id}/thumbnail.jpg");
    sqlx::query("INSERT INTO asset_file (id, assetId, type, path) VALUES (?, ?, 'thumbnail', ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(&bad_id)
        .bind(&thumb_path)
        .execute(app.pool())
        .await
        .unwrap();
    let stray = dir.join("objects").join("stray,untracked.bin");
    std::fs::write(&stray, b"stray").unwrap();
    let orphan = dir.join("thumbs").join("orphan");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("preview.jpg"), b"orphan").unwrap();
    for name in [
        "integrity-checksum-mismatch",
        "integrity-missing-files",
        "integrity-untracked-files",
    ] {
        let reply = call(
            &app,
            json_request(
                "POST",
                "/api/jobs",
                Some(&admin),
                Some(&format!(r#"{{"name":"{name}"}}"#)),
            ),
        )
        .await;
        assert_eq!(reply.status, 204, "{}", message(&reply.body));
    }
    let reply = call(
        &app,
        json_request("GET", "/api/admin/integrity/summary", Some(&admin), None),
    )
    .await;
    let summary = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{summary}");
    assert_eq!(summary["checksum_mismatch"], 1);
    assert_eq!(summary["missing_file"], 2);
    assert_eq!(summary["untracked_file"], 2);
    let reply = call(
        &app,
        json_request("GET", "/api/admin/integrity/report", Some(&admin), None),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/integrity/report?type=missing_file&limit=0",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/integrity/report?type=missing_file&limit=1",
            Some(&admin),
            None,
        ),
    )
    .await;
    let page = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["nextCursor"].as_str().unwrap().to_string();
    assert_ne!(page["items"][0]["id"].as_str().unwrap(), cursor);
    let reply = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/integrity/report?type=missing_file&limit=1&cursor={cursor}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    let next = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{next}");
    assert_eq!(next["items"][0]["id"], cursor);
    assert!(next.get("nextCursor").is_none());
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/integrity/report/untracked_file/csv",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type.as_deref(), Some("text/csv"));
    let csv = String::from_utf8(reply.body).unwrap();
    assert!(
        csv.starts_with("id,type,assetId,fileAssetId,path\n"),
        "{csv}"
    );
    assert!(csv.contains("\"stray,untracked.bin\""), "{csv}");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/integrity/report/checksum_mismatch/csv",
            Some(&admin),
            None,
        ),
    )
    .await;
    let checksum_csv = String::from_utf8(reply.body).unwrap();
    assert!(checksum_csv.contains(&bad_id), "{checksum_csv}");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/integrity/report/missing_file/csv",
            Some(&admin),
            None,
        ),
    )
    .await;
    let missing_csv = String::from_utf8(reply.body).unwrap();
    assert!(missing_csv.contains(&missing_id), "{missing_csv}");
    assert!(missing_csv.contains(&thumb_path), "{missing_csv}");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/jobs",
            Some(&admin),
            Some(r#"{"name":"integrity-missing-files-delete-all"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 204);
    let reply = call(
        &app,
        json_request("GET", "/api/admin/integrity/summary", Some(&admin), None),
    )
    .await;
    let summary = json_body(&reply.body);
    assert_eq!(summary["missing_file"], 0);
    assert_eq!(summary["checksum_mismatch"], 1);
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/integrity/report/not-a-type/csv",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 400);

    let now = "2020-01-02T03:04:05.000Z";
    let kept_group = Uuid::new_v4().to_string();
    let dropped_group = Uuid::new_v4().to_string();
    let deleted_face_group = Uuid::new_v4().to_string();
    for group in [&kept_group, &dropped_group, &deleted_face_group] {
        sqlx::query(
            "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
             VALUES (?, NULL, ?, ?, ?, ?)",
        )
        .bind(group)
        .bind(now)
        .bind(Uuid::now_v7().to_string())
        .bind(now)
        .bind(Uuid::now_v7().to_string())
        .execute(app.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO person (
                ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden,
                isFavorite, updateId
             ) VALUES (?, ?, ?, ?, '', '', 0, 0, ?)",
        )
        .bind(&admin_id)
        .bind(group)
        .bind(now)
        .bind(now)
        .bind(Uuid::now_v7().to_string())
        .execute(app.pool())
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO asset_face (
            id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
            boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, ownerId,
            assetVisibility
         ) VALUES (?, ?, ?, 16, 16, 0, 0, 4, 4, 'machine-learning', NULL, ?, ?, ?, 'timeline')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&bad_id)
    .bind(&kept_group)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .bind(&admin_id)
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO asset_face (
            id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
            boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, ownerId,
            assetVisibility
         ) VALUES (?, ?, ?, 16, 16, 0, 0, 4, 4, 'machine-learning', ?, ?, ?, ?, 'timeline')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&bad_id)
    .bind(&deleted_face_group)
    .bind(now)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .bind(&admin_id)
    .execute(app.pool())
    .await
    .unwrap();
    let cluster = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO cluster_group (id, name, createdAt, updatedAt, updateId) VALUES (?, NULL, ?, ?, ?)",
    )
    .bind(&cluster)
    .bind(now)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    let used_tag = Uuid::new_v4().to_string();
    let unused_tag = Uuid::new_v4().to_string();
    for (tag, value) in [(&used_tag, "used"), (&unused_tag, "unused")] {
        sqlx::query(
            "INSERT INTO tag (id, userId, value, createdAt, updatedAt, updateId) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(tag)
        .bind(&admin_id)
        .bind(value)
        .bind(now)
        .bind(now)
        .bind(Uuid::now_v7().to_string())
        .execute(app.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO tag_closure (id_ancestor, id_descendant) VALUES (?, ?)")
            .bind(tag)
            .bind(tag)
            .execute(app.pool())
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO tag_asset (assetId, tagId) VALUES (?, ?)")
        .bind(&bad_id)
        .bind(&used_tag)
        .execute(app.pool())
        .await
        .unwrap();
    let stale_memory = Uuid::new_v4().to_string();
    let saved_memory = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO memory (
            id, createdAt, updatedAt, ownerId, type, data, isSaved, memoryAt, updateId
         ) VALUES (?, '2000-01-01T00:00:00.000Z', ?, ?, 'on_this_day', '{}', 0, ?, ?)",
    )
    .bind(&stale_memory)
    .bind(now)
    .bind(&admin_id)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memory (
            id, createdAt, updatedAt, ownerId, type, data, isSaved, memoryAt, updateId
         ) VALUES (?, ?, ?, ?, 'on_this_day', '{}', 1, ?, ?)",
    )
    .bind(&saved_memory)
    .bind(now)
    .bind(now)
    .bind(&admin_id)
    .bind(now)
    .bind(Uuid::now_v7().to_string())
    .execute(app.pool())
    .await
    .unwrap();
    let old_user = create_user(&app, &admin, "old@example.com", "Old").await;
    let fresh_user = create_user(&app, &admin, "fresh@example.com", "Fresh").await;
    sqlx::query("UPDATE user SET deletedAt = '2000-01-01T00:00:00.000Z' WHERE id = ?")
        .bind(&old_user)
        .execute(app.pool())
        .await
        .unwrap();
    let recent = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    sqlx::query("UPDATE user SET deletedAt = ? WHERE id = ?")
        .bind(&recent)
        .bind(&fresh_user)
        .execute(app.pool())
        .await
        .unwrap();
    for name in [
        "person-cleanup",
        "tag-cleanup",
        "memory-cleanup",
        "memory-create",
        "user-cleanup",
    ] {
        let reply = call(
            &app,
            json_request(
                "POST",
                "/api/jobs",
                Some(&admin),
                Some(&format!(r#"{{"name":"{name}"}}"#)),
            ),
        )
        .await;
        assert_eq!(reply.status, 204, "{name} {}", message(&reply.body));
    }
    assert_eq!(count(&app, "SELECT COUNT(*) FROM person").await, 1);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM person_group").await, 1);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM cluster_group").await, 1);
    let tags: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tag WHERE id = ?")
        .bind(&used_tag)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(tags, 1);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM tag").await, 1);
    assert_eq!(
        count(
            &app,
            &format!("SELECT COUNT(*) FROM memory WHERE id = '{stale_memory}'")
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &app,
            &format!("SELECT COUNT(*) FROM memory WHERE id = '{saved_memory}'")
        )
        .await,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user WHERE id = ?")
            .bind(&old_user)
            .fetch_one(app.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user WHERE id = ?")
            .bind(&fresh_user)
            .fetch_one(app.pool())
            .await
            .unwrap(),
        1
    );

    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/jobs",
            Some(&admin),
            Some(r#"{"name":"backup-database"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 204, "{}", message(&reply.body));
    std::fs::write(dir.join("backups").join("extra.sql.gz"), b"not-gzip").unwrap();
    std::fs::write(dir.join("backups").join("oops.txt"), b"nope").unwrap();
    let reply = call(
        &app,
        json_request("GET", "/api/admin/database-backups", Some(&admin), None),
    )
    .await;
    let listed = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{listed}");
    let backups = listed["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 2);
    let filenames: Vec<_> = backups
        .iter()
        .map(|item| item["filename"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = filenames.clone();
    sorted.sort();
    sorted.reverse();
    assert_eq!(filenames, sorted);
    let backup_name = filenames
        .iter()
        .find(|name| name.ends_with(".sql") && !name.ends_with(".sql.gz"))
        .unwrap()
        .clone();
    assert!(backup_name.starts_with("immich-db-backup-"));
    assert!(backups
        .iter()
        .any(|item| item["filename"] == "extra.sql.gz"));
    assert!(backups.iter().all(|item| item["filename"] != "oops.txt"));
    let timezone = backups[0]["timezone"].as_str().unwrap();
    assert!(
        timezone.starts_with('+') || timezone.starts_with('-'),
        "{timezone}"
    );
    assert!(timezone.contains(':'), "{timezone}");
    let reply = call(
        &app,
        json_request(
            "GET",
            &format!("/api/admin/database-backups/{backup_name}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type.as_deref(), Some("application/sql"));
    assert!(reply.body.starts_with(b"SQLite format 3"), "backup header");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/database-backups/extra.sql.gz",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type.as_deref(), Some("application/gzip"));
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/database-backups/oops.txt",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Invalid backup name!");
    let reply = call(
        &app,
        json_request(
            "GET",
            "/api/admin/database-backups/missing-backup.sql",
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, 404);
    assert_eq!(message(&reply.body), "Not found");
    let reply = call(
        &app,
        json_request(
            "DELETE",
            "/api/admin/database-backups",
            Some(&admin),
            Some(&format!(r#"{{"backups":["oops.txt","{backup_name}"]}}"#)),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Invalid backup name!");
    assert!(dir.join("backups").join(&backup_name).is_file());
    let reply = call(
        &app,
        json_request(
            "DELETE",
            "/api/admin/database-backups",
            Some(&admin),
            Some(&format!(
                r#"{{"backups":["{backup_name}","extra.sql.gz","gone.sql"]}}"#
            )),
        ),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", message(&reply.body));
    let reply = call(
        &app,
        json_request("GET", "/api/admin/database-backups", Some(&admin), None),
    )
    .await;
    let listed = json_body(&reply.body);
    assert_eq!(listed["backups"].as_array().unwrap().len(), 0);

    let reply = call(
        &app,
        json_request("GET", "/api/admin/maintenance/status", None, None),
    )
    .await;
    let status = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{status}");
    assert_eq!(status["active"], false);
    assert_eq!(status["action"], "end");
    let reply = call(
        &app,
        json_request("POST", "/api/admin/maintenance/login", None, Some("{}")),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Not in maintenance mode");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance",
            Some(&admin),
            Some(r#"{"action":"end"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.is_empty());
    assert!(reply.cookies.is_empty());
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance",
            Some(&admin),
            Some(r#"{"action":"restore_database"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");
    let reply = call(
        &app,
        json_request("GET", "/api/admin/maintenance/status", None, None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["active"], false);
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance",
            Some(&admin),
            Some(r#"{"action":"start"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.is_empty());
    let token = cookie_value(&reply.cookies, "immich_maintenance_token").unwrap();
    let reply = call(
        &app,
        json_request("GET", "/api/admin/maintenance/status", None, None),
    )
    .await;
    let status = json_body(&reply.body);
    assert_eq!(status["active"], true);
    assert_eq!(status["action"], "start");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance/login",
            Some(&format!("immich_maintenance_token={token}")),
            None,
        ),
    )
    .await;
    let logged_in = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{logged_in}");
    assert_eq!(logged_in["username"], "Ada");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance/login",
            None,
            Some(r#"{"token":"wrong"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 401);
    assert_eq!(message(&reply.body), "Unauthorized");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance",
            Some(&admin),
            Some(r#"{"action":"end"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert!(reply
        .cookies
        .iter()
        .any(|cookie| cookie.contains("Max-Age=0")));
    let reply = call(
        &app,
        json_request("POST", "/api/admin/maintenance/login", None, Some("{}")),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Not in maintenance mode");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance",
            Some(&admin),
            Some(
                r#"{"action":"restore_database","restoreBackupFilename":"immich-db-backup-1.sql"}"#,
            ),
        ),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", message(&reply.body));
    let reply = call(
        &app,
        json_request("GET", "/api/users/me", Some(&admin), None),
    )
    .await;
    let me = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{me}");
    assert_eq!(me["name"], "Ada");
    assert_eq!(me["email"], "admin@example.com");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/maintenance",
            Some(&admin),
            Some(r#"{"action":"end"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    let reply = call(
        &app,
        json_request("GET", "/api/admin/maintenance/status", None, None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["active"], false);

    let reply = call(
        &app,
        json_request("GET", "/api/server/about", Some(&guest), None),
    )
    .await;
    let about = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{about}");
    assert_eq!(about["version"], "3.3.6");
    assert_eq!(about["licensed"], false);
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/server/license",
            Some(&guest),
            Some(
                &json!({"licenseKey": SERVER_LICENSE, "activationKey": SERVER_ACTIVATION})
                    .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(reply.status, 403);
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/server/license",
            Some(&admin),
            Some(r#"{"licenseKey":"NOPE-1","activationKey":"aa"}"#),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Invalid license key");
    let server_body =
        json!({"licenseKey": SERVER_LICENSE, "activationKey": SERVER_ACTIVATION}).to_string();
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/server/license",
            Some(&admin),
            Some(&server_body),
        ),
    )
    .await;
    let license = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{license}");
    assert_eq!(license["licenseKey"], SERVER_LICENSE);
    assert_eq!(license["activationKey"], SERVER_ACTIVATION);
    assert!(license["activatedAt"].as_str().unwrap().ends_with('Z'));
    let reply = call(
        &app,
        json_request("GET", "/api/server/about", Some(&admin), None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["licensed"], true);
    let reply = call(
        &app,
        json_request("GET", "/api/server/license", Some(&admin), None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["licenseKey"], SERVER_LICENSE);
    let reply = call(
        &app,
        json_request("GET", "/api/users/me", Some(&admin), None),
    )
    .await;
    assert!(json_body(&reply.body)["license"].is_null());
    let reply = call(
        &app,
        json_request("DELETE", "/api/server/license", Some(&admin), None),
    )
    .await;
    assert_eq!(reply.status, 204);
    let reply = call(
        &app,
        json_request("GET", "/api/server/license", Some(&admin), None),
    )
    .await;
    assert_eq!(reply.status, 404);
    assert_eq!(message(&reply.body), "Not found");
    let reply = call(
        &app,
        json_request("GET", "/api/server/about", Some(&admin), None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["licensed"], false);

    let user_body =
        json!({"licenseKey": USER_LICENSE, "activationKey": USER_ACTIVATION}).to_string();
    let reply = call(
        &app,
        json_request(
            "PUT",
            "/api/users/me/license",
            Some(&guest),
            Some(&user_body),
        ),
    )
    .await;
    let user_license = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{user_license}");
    assert_eq!(user_license["licenseKey"], USER_LICENSE);
    let reply = call(
        &app,
        json_request("GET", "/api/users/me", Some(&guest), None),
    )
    .await;
    let guest_me = json_body(&reply.body);
    assert_eq!(guest_me["license"]["licenseKey"], USER_LICENSE);
    let reply = call(
        &app,
        json_request("GET", "/api/users/me/license", Some(&guest), None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["activationKey"], USER_ACTIVATION);
    let reply = call(
        &app,
        json_request("DELETE", "/api/users/me/license", Some(&guest), None),
    )
    .await;
    assert_eq!(reply.status, 204);
    let reply = call(
        &app,
        json_request("GET", "/api/users/me", Some(&guest), None),
    )
    .await;
    assert!(json_body(&reply.body)["license"].is_null());
    let reply = call(
        &app,
        json_request("GET", "/api/users/me/license", Some(&guest), None),
    )
    .await;
    assert_eq!(reply.status, 404);

    let reply = call(
        &app,
        json_request("GET", "/api/server/features", None, None),
    )
    .await;
    assert_eq!(json_body(&reply.body)["email"], false);
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/notifications/test-email",
            Some(&admin),
            Some("{}"),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Failed to verify SMTP configuration");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/notifications/templates/welcome",
            Some(&admin),
            None,
        ),
    )
    .await;
    let template = json_body(&reply.body);
    assert_eq!(reply.status, 200, "{template}");
    assert_eq!(template["name"], "welcome");
    assert_eq!(template["html"], "<html><body><p>welcome</p></body></html>");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/notifications/templates/test",
            Some(&admin),
            Some(r#"{"template":"<b>hi</b>"}"#),
        ),
    )
    .await;
    assert_eq!(json_body(&reply.body)["html"], "<b>hi</b>");
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/notifications/templates/album-invite",
            Some(&admin),
            Some(r#"{"template":"a & b"}"#),
        ),
    )
    .await;
    assert_eq!(
        json_body(&reply.body)["html"],
        "<html><body><p>a &amp; b</p></body></html>"
    );
    let reply = call(
        &app,
        json_request(
            "POST",
            "/api/admin/notifications/templates/nope",
            Some(&admin),
            Some("{}"),
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(message(&reply.body), "Validation failed");
}
