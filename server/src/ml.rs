//! Client of the official machine-learning service. This process does not run a model.
//! A failed call does not fail the upload. CLIP is one little-endian float32 blob per Asset.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::db::{App, DbError};
use crate::system_config::clip_dimensions;

pub(crate) async fn discard_mismatched_clip(app: &App) -> Result<(), String> {
    let config = app
        .admin_config()
        .await
        .map_err(|error| format!("{error:?}"))?;
    let Some(model) = config["machineLearning"]["clip"]["modelName"].as_str() else {
        return Ok(());
    };
    let Some(dim) = clip_dimensions(model) else {
        return Ok(());
    };
    let expected = (dim * 4) as i64;
    sqlx::query("DELETE FROM asset_clip WHERE length(embedding) != ?")
        .bind(expected)
        .execute(app.pool())
        .await
        .map_err(|error| format!("{error}"))?;
    Ok(())
}

/// Encode the preview and store it when the configured model still matches.
pub(crate) async fn encode_asset_clip(
    app: &App,
    asset_id: &str,
    preview: &Path,
) -> Result<(), String> {
    let config = app
        .admin_config()
        .await
        .map_err(|error| format!("{error:?}"))?;
    if !smart_search_enabled(&config) {
        return Ok(());
    }
    let model = config["machineLearning"]["clip"]["modelName"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let Some(dim) = clip_dimensions(&model) else {
        return Ok(());
    };
    let image = tokio::fs::read(preview)
        .await
        .map_err(|error| error.to_string())?;
    let embedding = predict(&config, &image_request(&model), Some(&image), None).await?;
    let config = app
        .admin_config()
        .await
        .map_err(|error| format!("{error:?}"))?;
    let model_now = config["machineLearning"]["clip"]["modelName"]
        .as_str()
        .unwrap_or("");
    if model_now != model || embedding.len() != dim {
        return Ok(());
    }
    let bytes = floats_to_bytes(&embedding);
    sqlx::query(
        "INSERT INTO asset_clip (assetId, embedding) VALUES (?, ?)
         ON CONFLICT(assetId) DO UPDATE SET embedding = excluded.embedding",
    )
    .bind(asset_id)
    .bind(bytes)
    .execute(app.pool())
    .await
    .map_err(|error| format!("{error}"))?;
    Ok(())
}

/// Encode previews that have no stored vector. One pass at a time. A failure is logged
/// and the next asset is tried. Hidden assets are left alone, matching upload.
pub(crate) async fn encode_missing_clips(app: &App) {
    let Ok(_guard) = ml_pass().try_lock() else {
        eprintln!("machine learning already running");
        return;
    };
    encode_missing_clips_locked(app).await;
}

pub(crate) async fn encode_missing_clips_locked(app: &App) {
    let config = match app.admin_config().await {
        Ok(config) => config,
        Err(error) => {
            eprintln!("clip encode: {error:?}");
            return;
        }
    };
    if !smart_search_enabled(&config) {
        return;
    }
    let ids: Vec<String> = match sqlx::query_scalar(
        "SELECT asset.id FROM asset
         WHERE asset.deletedAt IS NULL
           AND asset.visibility != 'hidden'
           AND NOT EXISTS (
             SELECT 1 FROM asset_clip
             WHERE asset_clip.assetId = asset.id AND length(asset_clip.embedding) > 0
           )
         ORDER BY asset.createdAt",
    )
    .fetch_all(app.pool())
    .await
    {
        Ok(ids) => ids,
        Err(error) => {
            eprintln!("clip encode: {error}");
            return;
        }
    };
    let mut encoded = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    for id in ids {
        let preview = app.data_dir().join("thumbs").join(&id).join("preview.jpg");
        if !preview.is_file() {
            skipped += 1;
            continue;
        }
        if let Err(error) = encode_asset_clip(app, &id, &preview).await {
            eprintln!("clip encode {id}: {error}");
            failed += 1;
            continue;
        }
        let stored: Result<Option<i64>, sqlx::Error> =
            sqlx::query_scalar("SELECT length(embedding) FROM asset_clip WHERE assetId = ?")
                .bind(&id)
                .fetch_optional(app.pool())
                .await;
        match stored {
            Ok(Some(length)) if length > 0 => {
                encoded += 1;
                if let Err(error) = app.detect_duplicates(&id, false).await {
                    eprintln!("duplicate detection: {error:?}");
                }
            }
            Ok(_) => skipped += 1,
            Err(error) => {
                eprintln!("clip encode {id}: {error}");
                failed += 1;
            }
        }
    }
    if encoded > 0 || failed > 0 {
        eprintln!("clip encode finished encoded={encoded} skipped={skipped} failed={failed}");
    }
}

pub(crate) fn ml_pass() -> &'static tokio::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub(crate) fn smart_search_enabled(config: &Value) -> bool {
    config["machineLearning"]["enabled"]
        .as_bool()
        .unwrap_or(false)
        && config["machineLearning"]["clip"]["enabled"]
            .as_bool()
            .unwrap_or(false)
}

pub(crate) async fn encode_text(
    config: &Value,
    model: &str,
    text: &str,
    language: Option<&str>,
) -> Result<Vec<f32>, String> {
    predict(config, &text_request(model, language), None, Some(text)).await
}

pub(crate) async fn search_hits(
    app: &App,
    owner_id: &str,
    query: &[f32],
) -> Result<Vec<(String, f32)>, DbError> {
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT asset_clip.assetId, asset_clip.embedding
         FROM asset_clip
         JOIN asset ON asset.id = asset_clip.assetId
         WHERE asset.ownerId = ?
           AND asset.deletedAt IS NULL
           AND asset.visibility IN ('timeline', 'archive')",
    )
    .bind(owner_id)
    .fetch_all(app.pool())
    .await?;
    let mut hits = Vec::new();
    for (id, blob) in rows {
        let Some(embedding) = bytes_to_floats(&blob) else {
            continue;
        };
        if embedding.len() != query.len() {
            continue;
        }
        hits.push((id, cosine_distance(query, &embedding)));
    }
    hits.sort_by(|left, right| {
        left.1
            .total_cmp(&right.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(hits)
}

pub(crate) async fn asset_embedding(
    app: &App,
    owner_id: &str,
    asset_id: &str,
) -> Result<Option<Vec<f32>>, DbError> {
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT asset.id, COALESCE(asset_clip.embedding, X'')
         FROM asset
         LEFT JOIN asset_clip ON asset_clip.assetId = asset.id
         WHERE asset.id = ? AND asset.ownerId = ? AND asset.deletedAt IS NULL",
    )
    .bind(asset_id)
    .bind(owner_id)
    .fetch_optional(app.pool())
    .await?;
    let Some((_id, blob)) = row else {
        return Ok(None);
    };
    if blob.is_empty() {
        return Ok(Some(Vec::new()));
    }
    Ok(Some(bytes_to_floats(&blob).unwrap_or_default()))
}

fn image_request(model: &str) -> Value {
    json!({ "clip": { "visual": { "modelName": model } } })
}

fn text_request(model: &str, language: Option<&str>) -> Value {
    let mut textual = json!({ "modelName": model });
    if let Some(language) = language {
        textual["options"] = json!({ "language": language });
    }
    json!({ "clip": { "textual": textual } })
}

/// A ping uses the short availability timeout. Encoding a preview does not: the first
/// call can spend that time loading the model, and the official client does not abort it.
const PREDICT_TIMEOUT: Duration = Duration::from_secs(300);

async fn predict(
    config: &Value,
    entries: &Value,
    image: Option<&[u8]>,
    text: Option<&str>,
) -> Result<Vec<f32>, String> {
    let urls = ml_urls(config);
    if urls.is_empty() {
        return Err("no machine learning url".to_string());
    }
    let ping_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(availability_timeout(config)))
        .build()
        .map_err(|error| error.to_string())?;
    let predict_client = reqwest::Client::builder()
        .timeout(PREDICT_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let checks = config["machineLearning"]["availabilityChecks"]["enabled"]
        .as_bool()
        .unwrap_or(true);
    let mut healthy = Vec::new();
    let mut other = Vec::new();
    if checks {
        for url in &urls {
            if ping(&ping_client, url).await {
                healthy.push(url.clone());
            } else {
                other.push(url.clone());
            }
        }
    } else {
        healthy = urls;
    }
    let mut last_error = "Machine learning request failed for all URLs".to_string();
    for url in healthy.into_iter().chain(other) {
        match post_json(&predict_client, &url, entries, image, text).await {
            Ok(value) => match parse_clip_value(&value) {
                Ok(embedding) => return Ok(embedding),
                Err(error) => last_error = error,
            },
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

pub(crate) struct DetectedFace {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub embedding: Vec<f32>,
}

pub(crate) struct DetectedFaces {
    pub image_width: i64,
    pub image_height: i64,
    pub faces: Vec<DetectedFace>,
}

pub(crate) struct OcrLine {
    pub text: String,
    pub box_score: f64,
    pub text_score: f64,
    pub points: [f64; 8],
}

pub(crate) async fn detect_faces(
    config: &Value,
    model: &str,
    min_score: f64,
    image: &[u8],
) -> Result<DetectedFaces, String> {
    let entries = json!({
        "facial-recognition": {
            "detection": { "modelName": model, "options": { "minScore": min_score } },
            "recognition": { "modelName": model },
        }
    });
    let value = predict_json(config, &entries, Some(image), None).await?;
    parse_faces(&value)
}

pub(crate) async fn read_ocr(
    config: &Value,
    model: &str,
    min_detection: f64,
    min_recognition: f64,
    max_resolution: i64,
    image: &[u8],
) -> Result<Vec<OcrLine>, String> {
    let entries = json!({
        "ocr": {
            "detection": {
                "modelName": model,
                "options": { "minScore": min_detection, "maxResolution": max_resolution }
            },
            "recognition": {
                "modelName": model,
                "options": { "minScore": min_recognition }
            },
        }
    });
    let value = predict_json(config, &entries, Some(image), None).await?;
    parse_ocr(&value)
}

async fn predict_json(
    config: &Value,
    entries: &Value,
    image: Option<&[u8]>,
    text: Option<&str>,
) -> Result<Value, String> {
    let urls = ml_urls(config);
    if urls.is_empty() {
        return Err("no machine learning url".to_string());
    }
    let ping_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(availability_timeout(config)))
        .build()
        .map_err(|error| error.to_string())?;
    let predict_client = reqwest::Client::builder()
        .timeout(PREDICT_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let checks = config["machineLearning"]["availabilityChecks"]["enabled"]
        .as_bool()
        .unwrap_or(true);
    let mut healthy = Vec::new();
    let mut other = Vec::new();
    if checks {
        for url in &urls {
            if ping(&ping_client, url).await {
                healthy.push(url.clone());
            } else {
                other.push(url.clone());
            }
        }
    } else {
        healthy = urls;
    }
    let mut last_error = "Machine learning request failed for all URLs".to_string();
    for url in healthy.into_iter().chain(other) {
        match post_json(&predict_client, &url, entries, image, text).await {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn ml_urls(config: &Value) -> Vec<String> {
    config["machineLearning"]["urls"]
        .as_array()
        .map(|urls| {
            urls.iter()
                .filter_map(|url| url.as_str().map(str::trim).filter(|url| !url.is_empty()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn availability_timeout(config: &Value) -> u64 {
    config["machineLearning"]["availabilityChecks"]["timeout"]
        .as_u64()
        .filter(|timeout| *timeout > 0)
        .unwrap_or(2000)
}

async fn ping(client: &reqwest::Client, base: &str) -> bool {
    let Some(url) = ml_endpoint(base, "ping") else {
        return false;
    };
    matches!(client.get(url).send().await, Ok(response) if response.status().is_success())
}

async fn post_json(
    client: &reqwest::Client,
    base: &str,
    entries: &Value,
    image: Option<&[u8]>,
    text: Option<&str>,
) -> Result<Value, String> {
    let url =
        ml_endpoint(base, "predict").ok_or_else(|| format!("bad machine learning url {base}"))?;
    let boundary = multipart_boundary();
    let body = multipart_body(&boundary, entries, image, text);
    let response = client
        .post(url)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("machine learning status {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
fn parse_clip(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    parse_clip_value(&value)
}

fn parse_clip_value(value: &Value) -> Result<Vec<f32>, String> {
    let clip = value
        .get("clip")
        .ok_or("machine learning response has no clip")?;
    let numbers = if let Some(text) = clip.as_str() {
        serde_json::from_str::<Vec<f64>>(text).map_err(|error| error.to_string())?
    } else if let Some(array) = clip.as_array() {
        array
            .iter()
            .map(|item| {
                item.as_f64()
                    .ok_or_else(|| "clip value is not a number".to_string())
            })
            .collect::<Result<Vec<f64>, String>>()?
    } else {
        return Err("clip value is not a vector".to_string());
    };
    Ok(numbers.into_iter().map(|number| number as f32).collect())
}

fn parse_faces(value: &Value) -> Result<DetectedFaces, String> {
    let width = value
        .get("imageWidth")
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("imageWidth")
                .and_then(Value::as_f64)
                .map(|number| number as i64)
        })
        .ok_or("machine learning response has no image width")?;
    let height = value
        .get("imageHeight")
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("imageHeight")
                .and_then(Value::as_f64)
                .map(|number| number as i64)
        })
        .ok_or("machine learning response has no image height")?;
    let rows = value
        .get("facial-recognition")
        .and_then(Value::as_array)
        .ok_or("machine learning response has no faces")?;
    let mut faces = Vec::new();
    for row in rows {
        let box_value = row.get("boundingBox").ok_or("face has no bounding box")?;
        let embedding = vector_field(row.get("embedding").ok_or("face has no embedding")?)?;
        if embedding.is_empty() {
            continue;
        }
        faces.push(DetectedFace {
            x1: number_field(box_value, "x1")?,
            y1: number_field(box_value, "y1")?,
            x2: number_field(box_value, "x2")?,
            y2: number_field(box_value, "y2")?,
            embedding,
        });
    }
    Ok(DetectedFaces {
        image_width: width,
        image_height: height,
        faces,
    })
}

fn parse_ocr(value: &Value) -> Result<Vec<OcrLine>, String> {
    let ocr = value
        .get("ocr")
        .ok_or("machine learning response has no ocr")?;
    let texts = ocr
        .get("text")
        .and_then(Value::as_array)
        .ok_or("ocr response has no text")?;
    let boxes = ocr
        .get("box")
        .and_then(Value::as_array)
        .ok_or("ocr response has no box")?;
    let box_scores = ocr
        .get("boxScore")
        .and_then(Value::as_array)
        .ok_or("ocr response has no box score")?;
    let text_scores = ocr
        .get("textScore")
        .and_then(Value::as_array)
        .ok_or("ocr response has no text score")?;
    let mut lines = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        let Some(text) = text.as_str() else {
            continue;
        };
        let start = index * 8;
        if boxes.len() < start + 8 || index >= box_scores.len() || index >= text_scores.len() {
            continue;
        }
        let mut points = [0.0; 8];
        for (offset, point) in points.iter_mut().enumerate() {
            *point = boxes[start + offset]
                .as_f64()
                .ok_or("ocr box value is not a number")?;
        }
        lines.push(OcrLine {
            text: text.to_string(),
            box_score: box_scores[index].as_f64().unwrap_or(0.0),
            text_score: text_scores[index].as_f64().unwrap_or(0.0),
            points,
        });
    }
    Ok(lines)
}

fn number_field(value: &Value, name: &str) -> Result<f64, String> {
    value
        .get(name)
        .and_then(Value::as_f64)
        .or_else(|| {
            value
                .get(name)
                .and_then(Value::as_i64)
                .map(|number| number as f64)
        })
        .ok_or_else(|| format!("face box has no {name}"))
}

fn vector_field(value: &Value) -> Result<Vec<f32>, String> {
    let numbers = if let Some(text) = value.as_str() {
        serde_json::from_str::<Vec<f64>>(text).map_err(|error| error.to_string())?
    } else if let Some(array) = value.as_array() {
        array
            .iter()
            .map(|item| {
                item.as_f64()
                    .ok_or_else(|| "embedding value is not a number".to_string())
            })
            .collect::<Result<Vec<f64>, String>>()?
    } else {
        return Err("embedding value is not a vector".to_string());
    };
    Ok(numbers.into_iter().map(|number| number as f32).collect())
}

fn multipart_boundary() -> String {
    let mut random = [0u8; 8];
    if getrandom::fill(&mut random).is_err() {
        return "immichmlboundary".to_string();
    }
    format!("immichml{}", hex::encode(random))
}

fn multipart_body(
    boundary: &str,
    entries: &Value,
    image: Option<&[u8]>,
    text: Option<&str>,
) -> Vec<u8> {
    let mut body = Vec::new();
    let entries = entries.to_string();
    push_part(&mut body, boundary, "entries", None, entries.as_bytes());
    if let Some(image) = image {
        push_part(&mut body, boundary, "image", Some("preview.jpg"), image);
    }
    if let Some(text) = text {
        push_part(&mut body, boundary, "text", None, text.as_bytes());
    }
    body.extend(format!("--{boundary}--\r\n").into_bytes());
    body
}

fn push_part(body: &mut Vec<u8>, boundary: &str, name: &str, filename: Option<&str>, bytes: &[u8]) {
    body.extend(format!("--{boundary}\r\n").into_bytes());
    if let Some(filename) = filename {
        body.extend(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: image/jpeg\r\n\r\n"
            )
            .into_bytes(),
        );
    } else {
        body.extend(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").into_bytes(),
        );
    }
    body.extend_from_slice(bytes);
    body.extend(b"\r\n");
}

fn ml_endpoint(base: &str, relative: &str) -> Option<String> {
    let base = reqwest::Url::parse(base).ok()?;
    Some(base.join(relative).ok()?.to_string())
}

pub(crate) fn floats_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend(value.to_le_bytes());
    }
    bytes
}

pub(crate) fn bytes_to_floats(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

pub(crate) fn cosine_distance(left: &[f32], right: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (left, right) in left.iter().zip(right) {
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 1.0;
    }
    1.0 - dot / (left_norm.sqrt() * right_norm.sqrt())
}

#[cfg(test)]
mod tests {
    use super::{bytes_to_floats, cosine_distance, floats_to_bytes, ml_endpoint, parse_clip};
    use crate::system_config::clip_dimensions;

    #[test]
    fn clip_dimensions_follow_the_v336_table() {
        assert_eq!(clip_dimensions("ViT-B-32__openai"), Some(512));
        assert_eq!(clip_dimensions("org/ViT-B-32::openai"), Some(512));
        assert_eq!(clip_dimensions("RN50x4__openai"), Some(640));
        assert_eq!(
            clip_dimensions("ViT-SO400M-14-SigLIP-384__webli"),
            Some(1152)
        );
        assert_eq!(clip_dimensions("ViT-B-16-SigLIP2__webli"), Some(768));
        assert_eq!(clip_dimensions("ViT-L-16-SigLIP2-512__webli"), Some(1024));
        assert_eq!(
            clip_dimensions("ViT-gopt-16-SigLIP2-384__webli"),
            Some(1536)
        );
        assert_eq!(clip_dimensions("test-model"), None);
    }

    #[test]
    fn a_closer_vector_sorts_first_and_the_blob_round_trips() {
        let query = [1.0, 0.0];
        let near = cosine_distance(&query, &[0.9, 0.1]);
        let far = cosine_distance(&query, &[0.0, 1.0]);
        assert!(near < far);
        let bytes = floats_to_bytes(&[1.5, -2.0]);
        assert_eq!(bytes_to_floats(&bytes).unwrap(), vec![1.5, -2.0]);
    }

    #[test]
    fn the_clip_field_may_be_a_json_string() {
        let body = br#"{"clip":"[1.0, 0.0]","imageHeight":2,"imageWidth":2}"#;
        assert_eq!(parse_clip(body).unwrap(), vec![1.0, 0.0]);
        assert_eq!(
            ml_endpoint("http://ml.test:3003", "ping").as_deref(),
            Some("http://ml.test:3003/ping")
        );
        assert_eq!(
            ml_endpoint("http://ml.test:3003/", "predict").as_deref(),
            Some("http://ml.test:3003/predict")
        );
    }
}
