//! Originals live in object storage. Thumbnails stay on the Pi under the data directory.
//! When the S3 variables are unset, the same calls write `{data}/objects` so tests need no bucket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::config::{Config, S3Config};

type HmacSha256 = Hmac<Sha256>;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

pub enum Store {
    Local { root: PathBuf },
    S3(S3Store),
}

pub struct S3Store {
    config: S3Config,
    client: reqwest::Client,
    host: String,
    origin: String,
    https: bool,
}

impl Store {
    pub fn open(config: &Config) -> Result<Self, std::io::Error> {
        let objects = config.data_dir.join("objects");
        std::fs::create_dir_all(&objects)?;
        std::fs::create_dir_all(config.data_dir.join("thumbs"))?;
        std::fs::create_dir_all(config.data_dir.join("tmp"))?;
        if let Some(s3) = &config.s3 {
            return Ok(Self::S3(S3Store::new(s3.clone())?));
        }
        Ok(Self::Local { root: objects })
    }

    pub async fn put_file(
        &self,
        key: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<(), StoreError> {
        match self {
            Self::Local { root } => {
                let destination = local_path(root, key)?;
                if let Some(parent) = destination.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|error| StoreError(error.to_string()))?;
                }
                tokio::fs::copy(path, &destination)
                    .await
                    .map_err(|error| StoreError(error.to_string()))?;
                Ok(())
            }
            Self::S3(store) => store.put_file(key, path, content_type).await,
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), StoreError> {
        match self {
            Self::Local { root } => {
                let path = local_path(root, key)?;
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(StoreError(error.to_string())),
                }
            }
            Self::S3(store) => store.delete(key).await,
        }
    }

    /// Whole object. Missing keys are `None`. Used by the download zip, one file at a time.
    pub async fn read_file(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let response = match self
            .bytes_response(key, None, "application/octet-stream", "file")
            .await?
        {
            Some(response) => response,
            None => return Ok(None),
        };
        if response.status() != StatusCode::OK {
            return Err(StoreError(format!("object status {}", response.status())));
        }
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024 * 1024)
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        Ok(Some(bytes.to_vec()))
    }

    /// Byte response for an Original. `range` is the raw Range header. Missing keys are `None`.
    pub async fn bytes_response(
        &self,
        key: &str,
        range_header: Option<&str>,
        content_type: &str,
        filename: &str,
    ) -> Result<Option<Response>, StoreError> {
        match self {
            Self::Local { root } => {
                let path = local_path(root, key)?;
                let file = match tokio::fs::File::open(&path).await {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(StoreError(error.to_string())),
                };
                let len = file
                    .metadata()
                    .await
                    .map_err(|error| StoreError(error.to_string()))?
                    .len();
                file_response(file, len, range_header, content_type, filename).await
            }
            Self::S3(store) => {
                store
                    .bytes_response(key, range_header, content_type, filename)
                    .await
            }
        }
    }
}

fn local_path(root: &Path, key: &str) -> Result<PathBuf, StoreError> {
    if key.is_empty()
        || key
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(StoreError(format!("invalid object key {key}")));
    }
    Ok(root.join(key))
}

async fn file_response(
    mut file: tokio::fs::File,
    len: u64,
    range_header: Option<&str>,
    content_type: &str,
    filename: &str,
) -> Result<Option<Response>, StoreError> {
    let (status, start, end) = match range_header {
        Some(header) => match parse_range(header, len) {
            Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
            None => return Ok(Some(range_not_satisfiable(len))),
        },
        None => {
            if len == 0 {
                (StatusCode::OK, 0, 0)
            } else {
                (StatusCode::OK, 0, len - 1)
            }
        }
    };
    let slice_len = if len == 0 { 0 } else { end - start + 1 };
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|error| StoreError(error.to_string()))?;
    let reader = file.take(slice_len);
    let body = Body::from_stream(ReaderStream::new(reader));
    Ok(Some(build_response(
        body,
        ObjectSlice {
            status,
            slice_len,
            total: len,
            partial: range_header.is_some(),
            start,
            end,
        },
        content_type,
        filename,
    )))
}

struct ObjectSlice {
    status: StatusCode,
    slice_len: u64,
    total: u64,
    partial: bool,
    start: u64,
    end: u64,
}

fn build_response(body: Body, slice: ObjectSlice, content_type: &str, filename: &str) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = slice.status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&slice.slice_len.to_string()).expect("content-length"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(
            "private, max-age=86400, no-transform, stale-while-revalidate=2592000, stale-if-error=2592000",
        ),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(filename))
            .unwrap_or(HeaderValue::from_static("inline")),
    );
    if slice.partial {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!(
                "bytes {}-{}/{}",
                slice.start, slice.end, slice.total
            ))
            .expect("content-range"),
        );
    }
    response
}

fn range_not_satisfiable(len: u64) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    response.headers_mut().insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{len}")).expect("content-range"),
    );
    response
}

fn content_disposition(filename: &str) -> String {
    let cleaned: String = filename
        .chars()
        .map(|character| {
            if character == '"' || character == '\\' {
                '_'
            } else {
                character
            }
        })
        .collect();
    format!("inline; filename=\"{cleaned}\"")
}

/// `bytes=start-end`, `bytes=start-`, or `bytes=-suffix`. Returns inclusive bounds.
pub fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = header
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let suffix = suffix.min(len);
        return Some((len - suffix, len - 1));
    }
    let start: u64 = start.parse().ok()?;
    if start >= len {
        return None;
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().ok()?.min(len - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

impl S3Store {
    fn new(config: S3Config) -> Result<Self, std::io::Error> {
        let (https, host, origin) = split_endpoint(&config.endpoint)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60 * 60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(std::io::Error::other)?;
        Ok(Self {
            config,
            client,
            host,
            origin,
            https,
        })
    }

    fn request_host(&self) -> String {
        if self.config.path_style {
            self.host.clone()
        } else {
            format!("{}.{}", self.config.bucket, self.host)
        }
    }

    fn object_url(&self, key: &str) -> String {
        let encoded = encode_key(key);
        if self.config.path_style {
            format!(
                "{}/{}/{}",
                self.origin.trim_end_matches('/'),
                encode_segment(&self.config.bucket),
                encoded
            )
        } else {
            let scheme = if self.https { "https" } else { "http" };
            format!("{scheme}://{}.{}/{encoded}", self.config.bucket, self.host)
        }
    }

    fn canonical_uri(&self, key: &str) -> String {
        let encoded = encode_key(key);
        if self.config.path_style {
            format!("/{}/{}", encode_segment(&self.config.bucket), encoded)
        } else {
            format!("/{encoded}")
        }
    }

    fn authorization(
        &self,
        method: &str,
        canonical_uri: &str,
        payload_hash: &str,
        amz_date: &str,
    ) -> String {
        let date_stamp = &amz_date[..8];
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n",
            self.request_host()
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical = format!(
            "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let scope = format!("{date_stamp}/{}/s3/aws4_request", self.config.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical.as_bytes()))
        );
        let secret = format!("AWS4{}", self.config.secret_access_key);
        let date_key = hmac_sha256(secret.as_bytes(), date_stamp);
        let region_key = hmac_sha256(&date_key, &self.config.region);
        let service_key = hmac_sha256(&region_key, "s3");
        let signing_key = hmac_sha256(&service_key, "aws4_request");
        let signature = hex::encode(hmac_sha256(&signing_key, &string_to_sign));
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.config.access_key_id
        )
    }

    async fn put_file(&self, key: &str, path: &Path, content_type: &str) -> Result<(), StoreError> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        let len = file
            .metadata()
            .await
            .map_err(|error| StoreError(error.to_string()))?
            .len();
        let amz_date = amz_now();
        let uri = self.canonical_uri(key);
        let authorization = self.authorization("PUT", &uri, "UNSIGNED-PAYLOAD", &amz_date);
        let url = self.object_url(key);
        let response = self
            .client
            .put(url)
            .header(header::HOST, self.request_host())
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, len)
            .body(reqwest::Body::wrap_stream(ReaderStream::new(file)))
            .send()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StoreError(format!("s3 put {status}: {body}")));
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let amz_date = amz_now();
        let uri = self.canonical_uri(key);
        let authorization = self.authorization("DELETE", &uri, EMPTY_SHA256, &amz_date);
        let response = self
            .client
            .delete(self.object_url(key))
            .header(header::HOST, self.request_host())
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header(header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        if response.status().is_success() || response.status().as_u16() == 404 {
            return Ok(());
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(StoreError(format!("s3 delete {status}: {body}")))
    }

    async fn bytes_response(
        &self,
        key: &str,
        range_header: Option<&str>,
        content_type: &str,
        filename: &str,
    ) -> Result<Option<Response>, StoreError> {
        let head = self.head(key).await?;
        let Some(len) = head else {
            return Ok(None);
        };
        let (status, start, end) = match range_header {
            Some(header) => match parse_range(header, len) {
                Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
                None => return Ok(Some(range_not_satisfiable(len))),
            },
            None => {
                if len == 0 {
                    (StatusCode::OK, 0, 0)
                } else {
                    (StatusCode::OK, 0, len - 1)
                }
            }
        };
        let amz_date = amz_now();
        let uri = self.canonical_uri(key);
        let authorization = self.authorization("GET", &uri, EMPTY_SHA256, &amz_date);
        let mut request = self
            .client
            .get(self.object_url(key))
            .header(header::HOST, self.request_host())
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header(header::AUTHORIZATION, authorization);
        if range_header.is_some() && len > 0 {
            request = request.header(header::RANGE, format!("bytes={start}-{end}"));
        }
        let response = request
            .send()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() && response.status().as_u16() != 206 {
            let status_code = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StoreError(format!("s3 get {status_code}: {body}")));
        }
        let stream = response.bytes_stream();
        let slice_len = if len == 0 { 0 } else { end - start + 1 };
        Ok(Some(build_response(
            Body::from_stream(stream),
            ObjectSlice {
                status,
                slice_len,
                total: len,
                partial: range_header.is_some(),
                start,
                end,
            },
            content_type,
            filename,
        )))
    }

    async fn head(&self, key: &str) -> Result<Option<u64>, StoreError> {
        let amz_date = amz_now();
        let uri = self.canonical_uri(key);
        let authorization = self.authorization("HEAD", &uri, EMPTY_SHA256, &amz_date);
        let response = self
            .client
            .head(self.object_url(key))
            .header(header::HOST, self.request_host())
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header(header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StoreError(format!("s3 head {status}: {body}")));
        }
        let len = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Ok(Some(len))
    }
}

fn hmac_sha256(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn amz_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn split_endpoint(endpoint: &str) -> Result<(bool, String, String), String> {
    let (https, rest) = if let Some(rest) = endpoint.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = endpoint.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!("invalid IMMICH_S3_ENDPOINT {endpoint}"));
    };
    let host = rest.split('/').next().unwrap_or("").trim().to_string();
    if host.is_empty() {
        return Err(format!("invalid IMMICH_S3_ENDPOINT {endpoint}"));
    }
    let scheme = if https { "https" } else { "http" };
    let origin = format!("{scheme}://{host}");
    Ok((https, host, origin))
}

fn encode_key(key: &str) -> String {
    key.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
