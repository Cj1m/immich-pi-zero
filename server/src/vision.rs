//! Face boxes and OCR lines come from the machine-learning service, one asset at a time.
//! Grouping the same person across photos is cosine distance on this machine.
//! A user whose `clusterGroupId` is null is grouped only with their own faces: the
//! official filter `clusterGroupId = ?` matches nothing when that column is null.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::db::{new_id, new_update_id, now_iso, App};
use crate::ml;
use crate::people::PeopleError;

pub async fn run_pending(app: &App) {
    let Ok(_guard) = ml::ml_pass().try_lock() else {
        eprintln!("machine learning already running");
        return;
    };
    ml::encode_missing_clips_locked(app).await;
    detect_missing_faces_locked(app).await;
    recognize_unassigned_faces_locked(app).await;
    read_missing_ocr_locked(app).await;
}

impl App {
    pub async fn detect_missing_faces(&self) {
        let Ok(_guard) = ml::ml_pass().try_lock() else {
            eprintln!("machine learning already running");
            return;
        };
        detect_missing_faces_locked(self).await;
    }

    pub async fn recognize_unassigned_faces(&self) {
        let Ok(_guard) = ml::ml_pass().try_lock() else {
            eprintln!("machine learning already running");
            return;
        };
        recognize_unassigned_faces_locked(self).await;
    }

    pub async fn read_missing_ocr(&self) {
        let Ok(_guard) = ml::ml_pass().try_lock() else {
            eprintln!("machine learning already running");
            return;
        };
        read_missing_ocr_locked(self).await;
    }
}

struct FaceModel {
    model: String,
    min_score: f64,
    max_distance: f64,
    min_faces: usize,
}

struct OcrModel {
    model: String,
    min_detection: f64,
    min_recognition: f64,
    max_resolution: i64,
}

fn faces_enabled(config: &Value) -> bool {
    flag(config, "facialRecognition")
}

fn ocr_enabled(config: &Value) -> bool {
    flag(config, "ocr")
}

fn flag(config: &Value, name: &str) -> bool {
    let machine = &config["machineLearning"];
    machine["enabled"].as_bool().unwrap_or(false)
        && machine[name]["enabled"].as_bool().unwrap_or(false)
}

fn face_model(config: &Value) -> FaceModel {
    let face = &config["machineLearning"]["facialRecognition"];
    let min_faces = integer(&face["minFaces"], 3).max(1) as usize;
    FaceModel {
        model: text(&face["modelName"], "buffalo_l"),
        min_score: number(&face["minScore"], 0.7),
        max_distance: number(&face["maxDistance"], 0.5),
        min_faces,
    }
}

fn ocr_model(config: &Value) -> OcrModel {
    let ocr = &config["machineLearning"]["ocr"];
    OcrModel {
        model: text(&ocr["modelName"], "PP-OCRv5_mobile"),
        min_detection: number(&ocr["minDetectionScore"], 0.5),
        min_recognition: number(&ocr["minRecognitionScore"], 0.8),
        max_resolution: integer(&ocr["maxResolution"], 736),
    }
}

fn text(value: &Value, default: &str) -> String {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn number(value: &Value, default: f64) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|item| item as f64))
        .unwrap_or(default)
}

fn integer(value: &Value, default: i64) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|item| item as i64))
        .unwrap_or(default)
}

fn progress(kind: &str, count: u64) {
    if count > 0 && count % 25 == 0 {
        eprintln!("{kind} progress {count}");
    }
}

async fn load_config(app: &App) -> Option<Value> {
    match app.admin_config().await {
        Ok(config) => Some(config),
        Err(error) => {
            eprintln!("machine learning config: {error:?}");
            None
        }
    }
}

async fn detect_missing_faces_locked(app: &App) {
    let Some(config) = load_config(app).await else {
        return;
    };
    if !faces_enabled(&config) {
        return;
    }
    let settings = face_model(&config);
    let assets: Vec<DetectAsset> = match sqlx::query_as(
        "SELECT asset.id AS id, asset.ownerId AS owner_id, asset.visibility AS visibility,
                asset.deletedAt AS deleted_at
         FROM asset
         LEFT JOIN asset_job_status ON asset_job_status.assetId = asset.id
         WHERE asset.deletedAt IS NULL
           AND asset.visibility != 'hidden'
           AND asset_job_status.facesRecognizedAt IS NULL
         ORDER BY asset.fileCreatedAt, asset.id",
    )
    .fetch_all(app.pool())
    .await
    {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("face detection: {error:?}");
            return;
        }
    };
    let mut done = 0u64;
    let mut faces = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;
    for asset in assets {
        done += 1;
        progress("face detection", done);
        let Some(preview) = face_preview(app.data_dir(), &asset.id) else {
            skipped += 1;
            continue;
        };
        match detect_one(app, &config, &settings, &asset, &preview).await {
            Ok(added) => faces += added,
            Err(error) => {
                eprintln!("face detection {}: {error}", asset.id);
                failed += 1;
            }
        }
    }
    eprintln!(
        "face detection finished assets={done} faces={faces} skipped={skipped} failed={failed}"
    );
}

async fn detect_one(
    app: &App,
    config: &Value,
    settings: &FaceModel,
    asset: &DetectAsset,
    preview: &Path,
) -> Result<u64, String> {
    let image = tokio::fs::read(preview)
        .await
        .map_err(|error| error.to_string())?;
    let detected = ml::detect_faces(config, &settings.model, settings.min_score, &image).await?;
    let existing: Vec<StoredFace> = sqlx::query_as(
        "SELECT id, sourceType AS source_type, imageWidth AS image_width, imageHeight AS image_height,
                boundingBoxX1 AS x1, boundingBoxY1 AS y1, boundingBoxX2 AS x2, boundingBoxY2 AS y2
         FROM asset_face WHERE assetId = ? ORDER BY id",
    )
    .bind(&asset.id)
    .fetch_all(app.pool())
    .await
    .map_err(|error| format!("{error:?}"))?;
    let mut machine_ids: HashSet<String> = existing
        .iter()
        .filter(|face| face.source_type == "machine-learning")
        .map(|face| face.id.clone())
        .collect();
    let (width_scale, height_scale) =
        box_scale(&existing, detected.image_width, detected.image_height);
    let mut additions = Vec::new();
    let mut embeddings: Vec<(String, Vec<f32>)> = Vec::new();
    for face in &detected.faces {
        let scaled = (
            face.x1 * width_scale,
            face.y1 * height_scale,
            face.x2 * width_scale,
            face.y2 * height_scale,
        );
        if let Some(matched) = existing.iter().find(|stored| iou(stored, scaled) > 0.5) {
            // A manual or exif overlap keeps its box. Only its embedding is replaced.
            if !machine_ids.remove(&matched.id) {
                embeddings.push((matched.id.clone(), face.embedding.clone()));
            }
        } else {
            let id = new_id();
            embeddings.push((id.clone(), face.embedding.clone()));
            additions.push(NewFace {
                id,
                x1: round_px(face.x1),
                y1: round_px(face.y1),
                x2: round_px(face.x2),
                y2: round_px(face.y2),
            });
        }
    }
    let added = additions.len() as u64;
    let removed: Vec<String> = machine_ids.into_iter().collect();
    write_detected_faces(app, asset, &detected, &additions, &embeddings, &removed).await?;
    Ok(added)
}

/// The model's box is stored as returned. Scaling only lines it up with faces already on the asset.
fn box_scale(existing: &[StoredFace], image_width: i64, image_height: i64) -> (f64, f64) {
    let width = existing
        .first()
        .map(|face| face.image_width)
        .filter(|value| *value != 0)
        .unwrap_or(1);
    let height = existing
        .first()
        .map(|face| face.image_height)
        .filter(|value| *value != 0)
        .unwrap_or(1);
    (
        image_width as f64 / width as f64,
        image_height as f64 / height as f64,
    )
}

async fn write_detected_faces(
    app: &App,
    asset: &DetectAsset,
    detected: &ml::DetectedFaces,
    additions: &[NewFace],
    embeddings: &[(String, Vec<f32>)],
    removed: &[String],
) -> Result<(), String> {
    let now = now_iso();
    let mut transaction = app
        .pool()
        .begin()
        .await
        .map_err(|error| format!("{error:?}"))?;
    for face in additions {
        sqlx::query(
            "INSERT INTO asset_face (
                id, assetId, personGroupId, imageWidth, imageHeight, boundingBoxX1, boundingBoxY1,
                boundingBoxX2, boundingBoxY2, sourceType, deletedAt, updatedAt, updateId, isVisible,
                ownerId, assetVisibility, assetDeletedAt
             ) VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, 'machine-learning', NULL, ?, ?, 1, ?, ?, ?)",
        )
        .bind(&face.id)
        .bind(&asset.id)
        .bind(detected.image_width)
        .bind(detected.image_height)
        .bind(face.x1)
        .bind(face.y1)
        .bind(face.x2)
        .bind(face.y2)
        .bind(&now)
        .bind(new_update_id())
        .bind(&asset.owner_id)
        .bind(&asset.visibility)
        .bind(&asset.deleted_at)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("{error:?}"))?;
    }
    for (face_id, embedding) in embeddings {
        sqlx::query(
            "INSERT INTO face_search (faceId, embedding) VALUES (?, ?)
             ON CONFLICT(faceId) DO UPDATE SET embedding = excluded.embedding",
        )
        .bind(face_id)
        .bind(ml::floats_to_bytes(embedding))
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("{error:?}"))?;
    }
    for face_id in removed {
        sqlx::query(
            "INSERT INTO asset_face_audit (id, assetFaceId, assetId, deletedAt) VALUES (?, ?, ?, ?)",
        )
        .bind(new_update_id())
        .bind(face_id)
        .bind(&asset.id)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("{error:?}"))?;
        sqlx::query("DELETE FROM asset_face WHERE id = ?")
            .bind(face_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| format!("{error:?}"))?;
    }
    sqlx::query(
        "INSERT INTO asset_job_status (assetId, facesRecognizedAt) VALUES (?, ?)
         ON CONFLICT(assetId) DO UPDATE SET facesRecognizedAt = excluded.facesRecognizedAt",
    )
    .bind(&asset.id)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("{error:?}"))?;
    transaction
        .commit()
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn iou(face: &StoredFace, new_box: (f64, f64, f64, f64)) -> f64 {
    let x1 = (face.x1 as f64).max(new_box.0);
    let y1 = (face.y1 as f64).max(new_box.1);
    let x2 = (face.x2 as f64).min(new_box.2);
    let y2 = (face.y2 as f64).min(new_box.3);
    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_face = (face.x2 - face.x1) as f64 * (face.y2 - face.y1) as f64;
    let area_new = (new_box.2 - new_box.0) * (new_box.3 - new_box.1);
    let union = area_face + area_new - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn round_px(value: f64) -> i64 {
    if value.is_finite() {
        value.round() as i64
    } else {
        0
    }
}

fn face_preview(data_dir: &Path, asset_id: &str) -> Option<PathBuf> {
    let directory = data_dir.join("thumbs").join(asset_id);
    let edited = directory.join("preview_edited.jpg");
    if edited.is_file() {
        return Some(edited);
    }
    let preview = directory.join("preview.jpg");
    preview.is_file().then_some(preview)
}

fn ocr_preview(data_dir: &Path, asset_id: &str) -> Option<PathBuf> {
    let preview = data_dir.join("thumbs").join(asset_id).join("preview.jpg");
    preview.is_file().then_some(preview)
}

async fn recognize_unassigned_faces_locked(app: &App) {
    let Some(config) = load_config(app).await else {
        return;
    };
    if !faces_enabled(&config) {
        return;
    }
    let settings = face_model(&config);
    let rows: Vec<FaceRow> = match sqlx::query_as(
        "SELECT asset_face.id AS id, asset.ownerId AS owner_id,
                asset_face.personGroupId AS person_id, asset_face.sourceType AS source_type,
                face_search.embedding AS embedding, asset.visibility AS visibility,
                asset.fileCreatedAt AS file_created_at, user.clusterGroupId AS cluster_id
         FROM asset_face
         JOIN asset ON asset.id = asset_face.assetId
         JOIN user ON user.id = asset.ownerId
         JOIN face_search ON face_search.faceId = asset_face.id
         WHERE asset.deletedAt IS NULL AND asset_face.deletedAt IS NULL
           AND length(face_search.embedding) > 0",
    )
    .fetch_all(app.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("face recognition: {error:?}");
            return;
        }
    };
    let birth_rows: Vec<(String, String)> = match sqlx::query_as(
        "SELECT personGroupId, birthDate FROM person WHERE birthDate IS NOT NULL AND birthDate != ''",
    )
    .fetch_all(app.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("face recognition: {error:?}");
            return;
        }
    };
    let mut births: HashMap<String, Vec<String>> = HashMap::new();
    for (person_id, birth_date) in birth_rows {
        births.entry(person_id).or_default().push(birth_date);
    }
    let mut pools: HashMap<String, Vec<FacePoint>> = HashMap::new();
    let mut people: HashMap<String, HashMap<String, String>> = HashMap::new();
    for row in rows {
        let Some(embedding) = ml::bytes_to_floats(&row.embedding) else {
            continue;
        };
        if embedding.is_empty() {
            continue;
        }
        let key = match &row.cluster_id {
            Some(cluster) => format!("c:{cluster}"),
            None => format!("o:{}", row.owner_id),
        };
        if let Some(person_id) = row.person_id.clone() {
            people
                .entry(key.clone())
                .or_default()
                .insert(row.id.clone(), person_id);
        }
        pools.entry(key).or_default().push(FacePoint {
            id: row.id,
            owner_id: row.owner_id,
            source_type: row.source_type,
            embedding,
            visibility: row.visibility,
            file_created_at: row.file_created_at,
        });
    }
    let mut assigned = 0u64;
    let mut created = 0u64;
    for (key, pool) in &pools {
        let Some(person_of) = people.get_mut(key) else {
            let mut empty = HashMap::new();
            recognize_pool(
                app,
                pool,
                &mut empty,
                &births,
                &settings,
                &mut assigned,
                &mut created,
            )
            .await;
            continue;
        };
        recognize_pool(
            app,
            pool,
            person_of,
            &births,
            &settings,
            &mut assigned,
            &mut created,
        )
        .await;
    }
    eprintln!("face recognition finished assigned={assigned} people={created}");
}

async fn recognize_pool(
    app: &App,
    pool: &[FacePoint],
    person_of: &mut HashMap<String, String>,
    births: &HashMap<String, Vec<String>>,
    settings: &FaceModel,
    assigned: &mut u64,
    created: &mut u64,
) {
    let mut pending: Vec<usize> = pool
        .iter()
        .enumerate()
        .filter(|(_, face)| {
            face.source_type == "machine-learning" && !person_of.contains_key(&face.id)
        })
        .map(|(index, _)| index)
        .collect();
    pending.sort_by(|&left, &right| pool[left].id.cmp(&pool[right].id));
    let mut deferred = Vec::new();
    for index in pending {
        let close = close_faces(pool, index, person_of, births, settings.max_distance);
        if settings.min_faces > 1 && close.is_empty() {
            continue;
        }
        let core = pool[index].visibility == "timeline"
            && close.len() >= settings.min_faces.saturating_sub(1);
        if core {
            attach_core(app, pool, index, &close, person_of, assigned, created).await;
        } else {
            deferred.push(index);
        }
    }
    for index in deferred {
        let close = close_faces(pool, index, person_of, births, settings.max_distance);
        if settings.min_faces > 1 && close.is_empty() {
            continue;
        }
        if let Some(person_id) = person_in(&close, pool, person_of) {
            if assign_existing(app, pool, index, &person_id, person_of, assigned, created)
                .await
                .is_err()
            {
                eprintln!("face recognition {}: person {person_id}", pool[index].id);
            }
        }
    }
}

async fn attach_core(
    app: &App,
    pool: &[FacePoint],
    index: usize,
    close: &[usize],
    person_of: &mut HashMap<String, String>,
    assigned: &mut u64,
    created: &mut u64,
) {
    let face = &pool[index];
    let person_id = match person_in(close, pool, person_of) {
        Some(person_id) => person_id,
        None => match create_person_group(app, &face.owner_id).await {
            Ok(person_id) => person_id,
            Err(error) => {
                eprintln!("face recognition {}: {error:?}", face.id);
                return;
            }
        },
    };
    if assign_existing(app, pool, index, &person_id, person_of, assigned, created)
        .await
        .is_err()
    {
        eprintln!("face recognition {}: person {person_id}", face.id);
    }
}

async fn assign_existing(
    app: &App,
    pool: &[FacePoint],
    index: usize,
    person_id: &str,
    person_of: &mut HashMap<String, String>,
    assigned: &mut u64,
    created: &mut u64,
) -> Result<(), String> {
    let face = &pool[index];
    let made = ensure_owner_person(app, &face.owner_id, person_id, &face.id).await?;
    sqlx::query(
        "UPDATE asset_face SET personGroupId = ?, updatedAt = ?, updateId = ? WHERE id = ?",
    )
    .bind(person_id)
    .bind(now_iso())
    .bind(new_update_id())
    .bind(&face.id)
    .execute(app.pool())
    .await
    .map_err(|error| format!("{error:?}"))?;
    person_of.insert(face.id.clone(), person_id.to_string());
    *assigned += 1;
    progress("face recognition", *assigned);
    if made {
        *created += 1;
        if let Err(error) = app.refresh_feature_face(&face.owner_id, person_id).await {
            eprintln!("person thumbnail {person_id}: {}", people_error(&error));
        }
    }
    Ok(())
}

async fn create_person_group(app: &App, owner_id: &str) -> Result<String, String> {
    let id = new_id();
    let now = now_iso();
    let update_id = new_update_id();
    let cluster: Option<String> =
        sqlx::query_scalar("SELECT clusterGroupId FROM user WHERE id = ?")
            .bind(owner_id)
            .fetch_one(app.pool())
            .await
            .map_err(|error| format!("{error:?}"))?;
    sqlx::query(
        "INSERT INTO person_group (id, clusterGroupId, createdAt, createId, updatedAt, updateId)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(cluster)
    .bind(&now)
    .bind(&update_id)
    .bind(&now)
    .bind(&update_id)
    .execute(app.pool())
    .await
    .map_err(|error| format!("{error:?}"))?;
    Ok(id)
}

async fn ensure_owner_person(
    app: &App,
    owner_id: &str,
    person_id: &str,
    face_id: &str,
) -> Result<bool, String> {
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM person WHERE ownerId = ? AND personGroupId = ?")
            .bind(owner_id)
            .bind(person_id)
            .fetch_optional(app.pool())
            .await
            .map_err(|error| format!("{error:?}"))?;
    if exists.is_some() {
        return Ok(false);
    }
    let now = now_iso();
    sqlx::query(
        "INSERT INTO person
            (ownerId, personGroupId, createdAt, updatedAt, name, thumbnailPath, isHidden,
             birthDate, faceAssetId, isFavorite, color, updateId)
         VALUES (?, ?, ?, ?, '', '', 0, NULL, ?, 0, NULL, ?)",
    )
    .bind(owner_id)
    .bind(person_id)
    .bind(&now)
    .bind(&now)
    .bind(face_id)
    .bind(new_update_id())
    .execute(app.pool())
    .await
    .map_err(|error| format!("{error:?}"))?;
    Ok(true)
}

fn person_in(
    close: &[usize],
    pool: &[FacePoint],
    person_of: &HashMap<String, String>,
) -> Option<String> {
    close
        .iter()
        .find_map(|index| person_of.get(&pool[*index].id).cloned())
}

fn close_faces(
    pool: &[FacePoint],
    index: usize,
    person_of: &HashMap<String, String>,
    births: &HashMap<String, Vec<String>>,
    max_distance: f64,
) -> Vec<usize> {
    let face = &pool[index];
    let mut found = Vec::new();
    for (other_index, other) in pool.iter().enumerate() {
        if other_index == index || other.embedding.len() != face.embedding.len() {
            continue;
        }
        if let Some(person_id) = person_of.get(&other.id) {
            let born_later = births.get(person_id).is_some_and(|dates| {
                dates
                    .iter()
                    .any(|date| date.as_str() > face.file_created_at.as_str())
            });
            if born_later {
                continue;
            }
        }
        let distance = f64::from(ml::cosine_distance(&face.embedding, &other.embedding));
        if distance <= max_distance {
            found.push((distance, other_index));
        }
    }
    found.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| pool[left.1].id.cmp(&pool[right.1].id))
    });
    found.into_iter().map(|(_, index)| index).collect()
}

fn people_error(error: &PeopleError) -> String {
    match error {
        PeopleError::Db(error) => format!("{error:?}"),
        PeopleError::Status(_, text) => (*text).to_string(),
    }
}

async fn read_missing_ocr_locked(app: &App) {
    let Some(config) = load_config(app).await else {
        return;
    };
    if !ocr_enabled(&config) {
        return;
    }
    let settings = ocr_model(&config);
    let ids: Vec<String> = match sqlx::query_scalar(
        "SELECT asset.id FROM asset
         LEFT JOIN asset_job_status ON asset_job_status.assetId = asset.id
         WHERE asset.deletedAt IS NULL
           AND asset.visibility != 'hidden'
           AND asset_job_status.ocrAt IS NULL
         ORDER BY asset.fileCreatedAt, asset.id",
    )
    .fetch_all(app.pool())
    .await
    {
        Ok(ids) => ids,
        Err(error) => {
            eprintln!("ocr: {error:?}");
            return;
        }
    };
    let mut done = 0u64;
    let mut lines = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;
    for id in ids {
        done += 1;
        progress("ocr", done);
        let Some(preview) = ocr_preview(app.data_dir(), &id) else {
            skipped += 1;
            continue;
        };
        match read_one(app, &config, &settings, &id, &preview).await {
            Ok(count) => lines += count,
            Err(error) => {
                eprintln!("ocr {id}: {error}");
                failed += 1;
            }
        }
    }
    eprintln!("ocr finished assets={done} lines={lines} skipped={skipped} failed={failed}");
}

async fn read_one(
    app: &App,
    config: &Value,
    settings: &OcrModel,
    asset_id: &str,
    preview: &Path,
) -> Result<u64, String> {
    let image = tokio::fs::read(preview)
        .await
        .map_err(|error| error.to_string())?;
    let lines = ml::read_ocr(
        config,
        &settings.model,
        settings.min_detection,
        settings.min_recognition,
        settings.max_resolution,
        &image,
    )
    .await?;
    let now = now_iso();
    let mut transaction = app
        .pool()
        .begin()
        .await
        .map_err(|error| format!("{error:?}"))?;
    sqlx::query("DELETE FROM asset_ocr WHERE assetId = ?")
        .bind(asset_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("{error:?}"))?;
    for line in &lines {
        sqlx::query(
            "INSERT INTO asset_ocr (
                id, assetId, x1, y1, x2, y2, x3, y3, x4, y4, boxScore, textScore, text,
                isVisible, updatedAt, updateId
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(new_id())
        .bind(asset_id)
        .bind(line.points[0])
        .bind(line.points[1])
        .bind(line.points[2])
        .bind(line.points[3])
        .bind(line.points[4])
        .bind(line.points[5])
        .bind(line.points[6])
        .bind(line.points[7])
        .bind(line.box_score)
        .bind(line.text_score)
        .bind(&line.text)
        .bind(&now)
        .bind(new_update_id())
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("{error:?}"))?;
    }
    sqlx::query(
        "INSERT INTO asset_job_status (assetId, ocrAt) VALUES (?, ?)
         ON CONFLICT(assetId) DO UPDATE SET ocrAt = excluded.ocrAt",
    )
    .bind(asset_id)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("{error:?}"))?;
    transaction
        .commit()
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(lines.len() as u64)
}

#[derive(sqlx::FromRow)]
struct DetectAsset {
    id: String,
    owner_id: String,
    visibility: String,
    deleted_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct StoredFace {
    id: String,
    source_type: String,
    image_width: i64,
    image_height: i64,
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
}

struct NewFace {
    id: String,
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
}

#[derive(sqlx::FromRow)]
struct FaceRow {
    id: String,
    owner_id: String,
    person_id: Option<String>,
    source_type: String,
    embedding: Vec<u8>,
    visibility: String,
    file_created_at: String,
    cluster_id: Option<String>,
}

struct FacePoint {
    id: String,
    owner_id: String,
    source_type: String,
    embedding: Vec<f32>,
    visibility: String,
    file_created_at: String,
}
