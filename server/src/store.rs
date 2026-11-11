//! Originals live in object storage. Thumbnails stay on the Pi under the data directory.
//! When the S3 variables are unset, the same calls write `{data}/objects` so tests need no bucket.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use chrono::Utc;
use futures_util::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
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
    S3(Box<S3Store>),
}

pub struct S3Store {
    config: S3Config,
    client: reqwest::Client,
    host: String,
    origin: String,
    https: bool,
    data_dir: PathBuf,
    object_key: crate::seal::ObjectKey,
}

impl Store {
    pub fn open(config: &Config) -> Result<Self, std::io::Error> {
        let objects = config.data_dir.join("objects");
        std::fs::create_dir_all(&objects)?;
        std::fs::create_dir_all(config.data_dir.join("thumbs"))?;
        std::fs::create_dir_all(config.data_dir.join("tmp"))?;
        std::fs::create_dir_all(config.data_dir.join("spool"))?;
        if let Some(s3) = &config.s3 {
            return Ok(Self::S3(Box::new(S3Store::new(
                s3.clone(),
                &config.data_dir,
            )?)));
        }
        Ok(Self::Local { root: objects })
    }

    /// S3 originals are sealed and uploaded after the phone has its 201.
    pub(crate) fn defers_upload(&self) -> bool {
        matches!(self, Self::S3(_))
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

    /// Copy one original onto `dest` as plaintext. Missing keys are `Ok(false)`.
    /// A sealed S3 object is decrypted while it is copied. The local store is copied as-is.
    pub async fn export_file(&self, key: &str, dest: &Path) -> Result<bool, StoreError> {
        match self {
            Self::Local { root } => {
                let path = local_path(root, key)?;
                if !path.is_file() {
                    return Ok(false);
                }
                if path == dest {
                    return Ok(true);
                }
                if let Some(parent) = dest.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|error| StoreError(error.to_string()))?;
                }
                tokio::fs::copy(&path, dest)
                    .await
                    .map_err(|error| StoreError(error.to_string()))?;
                Ok(true)
            }
            Self::S3(store) => store.export_file(key, dest).await,
        }
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

pub(crate) async fn plain_file_response(
    path: &Path,
    range_header: Option<&str>,
    content_type: &str,
    filename: &str,
) -> Result<Option<Response>, StoreError> {
    let file = match tokio::fs::File::open(path).await {
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
    fn new(config: S3Config, data_dir: &Path) -> Result<Self, std::io::Error> {
        let (https, host, origin) = split_endpoint(&config.endpoint)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60 * 60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(std::io::Error::other)?;
        let object_key = crate::seal::ObjectKey::load_or_create(&data_dir.join("keys"))?;
        Ok(Self {
            config,
            client,
            host,
            origin,
            https,
            data_dir: data_dir.to_path_buf(),
            object_key,
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

    async fn put_file(
        &self,
        key: &str,
        path: &Path,
        _content_type: &str,
    ) -> Result<(), StoreError> {
        let plain_len = tokio::fs::metadata(path)
            .await
            .map_err(|error| StoreError(error.to_string()))?
            .len();
        let sealed_path = self
            .data_dir
            .join("tmp")
            .join(format!("seal-{}", uuid::Uuid::new_v4()));
        if let Some(parent) = sealed_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| StoreError(error.to_string()))?;
        }
        let object_key = self.object_key.clone();
        let source = path.to_path_buf();
        let destination = sealed_path.clone();
        tokio::task::spawn_blocking(move || object_key.seal_file(&source, &destination))
            .await
            .map_err(|error| StoreError(error.to_string()))?
            .map_err(|error| StoreError(error.to_string()))?;
        let guard = DeletePath(sealed_path);
        let file = tokio::fs::File::open(&guard.0)
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
            .header(header::CONTENT_TYPE, "application/octet-stream")
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
        eprintln!("sealed {key} ({plain_len} bytes)");
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
        if len >= crate::seal::HEADER_LEN as u64 {
            let header = self
                .read_range(key, 0, crate::seal::HEADER_LEN as u64 - 1)
                .await?;
            if crate::seal::looks_sealed(&header) {
                return self
                    .sealed_response(key, len, &header, range_header, content_type, filename)
                    .await;
            }
        }
        self.plain_response(key, len, range_header, content_type, filename)
            .await
    }

    async fn export_file(&self, key: &str, dest: &Path) -> Result<bool, StoreError> {
        let Some(len) = self.head(key).await? else {
            return Ok(false);
        };
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| StoreError(error.to_string()))?;
        }
        let copied = self.export_body(key, len, dest).await;
        if copied.is_err() {
            let _ = tokio::fs::remove_file(dest).await;
        }
        copied.map(|()| true)
    }

    async fn export_body(&self, key: &str, len: u64, dest: &Path) -> Result<(), StoreError> {
        if len >= crate::seal::HEADER_LEN as u64 {
            let header = self
                .read_range(key, 0, crate::seal::HEADER_LEN as u64 - 1)
                .await?;
            if crate::seal::looks_sealed(&header) {
                let opened = self
                    .object_key
                    .open_header(&header)
                    .map_err(|error| StoreError(error.0))?;
                let expected = crate::seal::sealed_len(opened.plain_len, opened.chunk_size)
                    .ok_or_else(|| StoreError("sealed object length overflow".to_string()))?;
                if len != expected {
                    return Err(StoreError(format!(
                        "sealed object {key} is {len} bytes, header says {expected}"
                    )));
                }
                if opened.plain_len == 0 {
                    tokio::fs::write(dest, [])
                        .await
                        .map_err(|error| StoreError(error.to_string()))?;
                    return Ok(());
                }
                return self
                    .write_plain(key, &opened, 0, opened.plain_len - 1, dest)
                    .await;
            }
        }
        self.download_plain(key, dest).await
    }

    async fn download_plain(&self, key: &str, dest: &Path) -> Result<(), StoreError> {
        let amz_date = amz_now();
        let uri = self.canonical_uri(key);
        let authorization = self.authorization("GET", &uri, EMPTY_SHA256, &amz_date);
        let response = self
            .client
            .get(self.object_url(key))
            .header(header::HOST, self.request_host())
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header(header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StoreError(format!("s3 get {status}: {body}")));
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| StoreError(error.to_string()))?;
            file.write_all(&chunk)
                .await
                .map_err(|error| StoreError(error.to_string()))?;
        }
        file.flush()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        Ok(())
    }

    async fn plain_response(
        &self,
        key: &str,
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

    async fn sealed_response(
        &self,
        key: &str,
        cipher_len: u64,
        header: &[u8],
        range_header: Option<&str>,
        content_type: &str,
        filename: &str,
    ) -> Result<Option<Response>, StoreError> {
        let opened = self
            .object_key
            .open_header(header)
            .map_err(|error| StoreError(error.0))?;
        let expected = crate::seal::sealed_len(opened.plain_len, opened.chunk_size)
            .ok_or_else(|| StoreError("sealed object length overflow".to_string()))?;
        if cipher_len != expected {
            return Err(StoreError(format!(
                "sealed object {key} is {cipher_len} bytes, header says {expected}"
            )));
        }
        let (status, start, end) = match range_header {
            Some(header) => match parse_range(header, opened.plain_len) {
                Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
                None => return Ok(Some(range_not_satisfiable(opened.plain_len))),
            },
            None => {
                if opened.plain_len == 0 {
                    (StatusCode::OK, 0, 0)
                } else {
                    (StatusCode::OK, 0, opened.plain_len - 1)
                }
            }
        };
        let total = opened.plain_len;
        let slice_len = if opened.plain_len == 0 {
            0
        } else {
            end - start + 1
        };
        // Headers go out once Backblaze accepts the range. Chunks follow as they arrive,
        // so a player can start before the rest of the file is on the Pi.
        let body = if slice_len == 0 {
            Body::empty()
        } else {
            let span = chunk_span(&opened, start, end)?;
            let response = self
                .open_range(key, span.cipher_start, span.cipher_end)
                .await?;
            Body::from_stream(plain_chunks(BytePump::new(response), opened, start, end))
        };
        Ok(Some(build_response(
            body,
            ObjectSlice {
                status,
                slice_len,
                total,
                partial: range_header.is_some(),
                start,
                end,
            },
            content_type,
            filename,
        )))
    }

    async fn write_plain(
        &self,
        key: &str,
        opened: &crate::seal::Opened,
        start: u64,
        end: u64,
        dest: &Path,
    ) -> Result<(), StoreError> {
        let span = chunk_span(opened, start, end)?;
        let response = self
            .open_range(key, span.cipher_start, span.cipher_end)
            .await?;
        let mut pump = BytePump::new(response);
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        for index in span.first..=span.last {
            let plain_len =
                crate::seal::chunk_plain_len(opened.plain_len, opened.chunk_size, index)
                    .map_err(|error| StoreError(error.0))?;
            let wire = pump.read_exact((plain_len + 16) as usize).await?;
            let plain = opened
                .decrypt_chunk(index, &wire)
                .map_err(|error| StoreError(error.0))?;
            let piece = crate::seal::overlap(index, opened.chunk_size, start, end, &plain);
            file.write_all(piece)
                .await
                .map_err(|error| StoreError(error.to_string()))?;
        }
        file.flush()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        Ok(())
    }

    async fn read_range(
        &self,
        key: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<Vec<u8>, StoreError> {
        let response = self.open_range(key, start, end_inclusive).await?;
        let mut pump = BytePump::new(response);
        let count = (end_inclusive - start + 1) as usize;
        pump.read_exact(count).await
    }

    async fn open_range(
        &self,
        key: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<reqwest::Response, StoreError> {
        let amz_date = amz_now();
        let uri = self.canonical_uri(key);
        let authorization = self.authorization("GET", &uri, EMPTY_SHA256, &amz_date);
        let response = self
            .client
            .get(self.object_url(key))
            .header(header::HOST, self.request_host())
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", EMPTY_SHA256)
            .header(header::AUTHORIZATION, authorization)
            .header(header::RANGE, format!("bytes={start}-{end_inclusive}"))
            .send()
            .await
            .map_err(|error| StoreError(error.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(StoreError(format!("s3 get {key} returned 404")));
        }
        if !response.status().is_success() && response.status().as_u16() != 206 {
            let status_code = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StoreError(format!("s3 get {status_code}: {body}")));
        }
        Ok(response)
    }
}

struct ChunkSpan {
    first: u64,
    last: u64,
    cipher_start: u64,
    cipher_end: u64,
}

fn chunk_span(opened: &crate::seal::Opened, start: u64, end: u64) -> Result<ChunkSpan, StoreError> {
    let chunk = u64::from(opened.chunk_size);
    let first = start / chunk;
    let last = end / chunk;
    let cipher_start = crate::seal::chunk_offset(opened.chunk_size, first)
        .map_err(|error| StoreError(error.0))?;
    let last_plain = crate::seal::chunk_plain_len(opened.plain_len, opened.chunk_size, last)
        .map_err(|error| StoreError(error.0))?;
    let last_offset = crate::seal::chunk_offset(opened.chunk_size, last)
        .map_err(|error| StoreError(error.0))?;
    let cipher_end = last_offset + last_plain + 16 - 1;
    Ok(ChunkSpan {
        first,
        last,
        cipher_start,
        cipher_end,
    })
}

struct ChunkRead {
    pump: BytePump,
    opened: crate::seal::Opened,
    index: u64,
    last: u64,
    start: u64,
    end: u64,
}

fn plain_chunks(
    pump: BytePump,
    opened: crate::seal::Opened,
    start: u64,
    end: u64,
) -> impl Stream<Item = Result<Vec<u8>, std::io::Error>> + Send {
    let chunk = u64::from(opened.chunk_size);
    let state = ChunkRead {
        pump,
        opened,
        index: start / chunk,
        last: end / chunk,
        start,
        end,
    };
    futures_util::stream::try_unfold(state, |mut state| async move {
        if state.index > state.last {
            return Ok(None);
        }
        let index = state.index;
        let plain_len =
            crate::seal::chunk_plain_len(state.opened.plain_len, state.opened.chunk_size, index)
                .map_err(|error| std::io::Error::other(error.0))?;
        let wire = state
            .pump
            .read_exact((plain_len + 16) as usize)
            .await
            .map_err(|error| std::io::Error::other(error.0))?;
        let plain = state
            .opened
            .decrypt_chunk(index, &wire)
            .map_err(|error| std::io::Error::other(error.0))?;
        let piece = crate::seal::overlap(
            index,
            state.opened.chunk_size,
            state.start,
            state.end,
            &plain,
        )
        .to_vec();
        state.index += 1;
        Ok(Some((piece, state)))
    })
}

struct DeletePath(PathBuf);

impl Drop for DeletePath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct BytePump {
    stream: Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>>,
    pending: Vec<u8>,
    pending_pos: usize,
}

impl BytePump {
    fn new(response: reqwest::Response) -> Self {
        let stream = response.bytes_stream().map(|item| {
            item.map(|chunk| chunk.as_ref().to_vec())
                .map_err(|error| error.to_string())
        });
        Self::from_stream(stream)
    }

    fn from_stream(stream: impl Stream<Item = Result<Vec<u8>, String>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            pending: Vec::new(),
            pending_pos: 0,
        }
    }

    async fn read_exact(&mut self, count: usize) -> Result<Vec<u8>, StoreError> {
        let mut out = vec![0u8; count];
        let mut filled = 0;
        while filled < count {
            if self.pending_pos >= self.pending.len() {
                match self.stream.as_mut().next().await {
                    Some(Ok(chunk)) => {
                        self.pending = chunk;
                        self.pending_pos = 0;
                    }
                    Some(Err(error)) => return Err(StoreError(error)),
                    None => return Err(StoreError("sealed object ended early".to_string())),
                }
                if self.pending.is_empty() {
                    continue;
                }
            }
            let take = (count - filled).min(self.pending.len() - self.pending_pos);
            out[filled..filled + take]
                .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + take]);
            self.pending_pos += take;
            filled += take;
        }
        Ok(out)
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

#[cfg(test)]
mod tests {
    use super::{plain_chunks, BytePump};
    use futures_util::StreamExt;
    use std::time::Duration;

    fn sealed_wires(plain: &[u8], chunk: u32) -> (crate::seal::Opened, Vec<Vec<u8>>) {
        let dir = std::env::temp_dir().join(format!(
            "immich-seal-stream-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = crate::seal::ObjectKey::load_or_create(&dir.join("keys")).unwrap();
        let src = dir.join("plain");
        let dst = dir.join("sealed");
        std::fs::write(&src, plain).unwrap();
        key.seal_file_chunked(&src, &dst, chunk).unwrap();
        let sealed = std::fs::read(&dst).unwrap();
        let opened = key
            .open_header(&sealed[..crate::seal::HEADER_LEN])
            .unwrap();
        let body = &sealed[crate::seal::HEADER_LEN..];
        let wire = chunk as usize + 16;
        let wires = body.chunks(wire).map(|chunk| chunk.to_vec()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        (opened, wires)
    }

    fn wire_source(
        rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) -> impl futures_util::Stream<Item = Result<Vec<u8>, String>> + Send {
        futures_util::stream::unfold(rx, |mut rx| async move {
            let chunk = rx.recv().await?;
            Some((Ok(chunk), rx))
        })
    }

    #[tokio::test]
    async fn playback_yields_the_first_chunk_before_the_rest_arrives() {
        let plain: Vec<u8> = (0..24).collect();
        let (opened, wires) = sealed_wires(&plain, 8);
        assert_eq!(wires.len(), 3);
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(wires[0].clone()).await.unwrap();
        let mut stream = std::pin::pin!(plain_chunks(
            BytePump::from_stream(wire_source(rx)),
            opened,
            0,
            23,
        ));
        let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("first chunk")
            .unwrap()
            .unwrap();
        assert_eq!(first, plain[..8]);
        let waiting = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;
        assert!(waiting.is_err(), "later chunks were read early");
        tx.send(wires[1].clone()).await.unwrap();
        tx.send(wires[2].clone()).await.unwrap();
        drop(tx);
        let second = stream.next().await.unwrap().unwrap();
        let third = stream.next().await.unwrap().unwrap();
        assert!(stream.next().await.is_none());
        assert_eq!([first, second, third].concat(), plain);
    }

    #[tokio::test]
    async fn a_tail_range_does_not_wait_for_earlier_chunks() {
        let plain: Vec<u8> = (0..24).collect();
        let (opened, wires) = sealed_wires(&plain, 8);
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(wires[2].clone()).await.unwrap();
        drop(tx);
        let mut stream = std::pin::pin!(plain_chunks(
            BytePump::from_stream(wire_source(rx)),
            opened,
            16,
            23,
        ));
        let tail = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("tail chunk")
            .unwrap()
            .unwrap();
        assert_eq!(tail, plain[16..]);
        assert!(stream.next().await.is_none());
    }
}
