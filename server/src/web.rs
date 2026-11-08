//! The official web build, when `IMMICH_WEB_ROOT` contains it.
//! `/s/{slug}` and `/share/{key}` are that `index.html`, with the link preview filled in.

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::db::App;
use crate::http::message;
use crate::shares::DocumentLink;

pub(crate) async fn browser_document(State(app): State<App>, request: Request) -> Response {
    let method = request.method().as_str();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(str::to_string);
    let full = match &query {
        Some(query) => format!("{path}?{query}"),
        None => path.clone(),
    };
    if let Some(response) = static_file(&app, &path).await {
        return response;
    }
    if path.starts_with("/api") || (method != "GET" && method != "HEAD") {
        return message(
            StatusCode::NOT_FOUND,
            format!("Cannot {} {path}", request.method()),
        );
    }
    let accept = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("*/*");
    if !accepts_html(accept) {
        return message(
            StatusCode::NOT_ACCEPTABLE,
            format!("The route {path} was requested as {accept}, but only returns text/html"),
        );
    }
    let mut status = StatusCode::OK;
    let mut html = tokio::fs::read_to_string(app.web_root().join("index.html"))
        .await
        .unwrap_or_default();
    if let Some((by_key, token)) = share_capture(&full) {
        let origin = origin(&request);
        match app.document_link(by_key, token, &origin).await {
            Ok(DocumentLink::Missing) => status = StatusCode::NOT_FOUND,
            Ok(DocumentLink::Quiet) => {}
            Ok(DocumentLink::Tagged {
                title,
                description,
                image_url,
            }) => html = render_tags(&html, &title, &description, &image_url),
            Err(error) => {
                eprintln!("shared link document: {error:?}");
                status = StatusCode::NOT_FOUND;
            }
        }
    }
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn share_capture(url: &str) -> Option<(bool, &str)> {
    if let Some(token) = url.strip_prefix("/share/") {
        if !token.is_empty() {
            return Some((true, token));
        }
    }
    if let Some(token) = url.strip_prefix("/s/") {
        if !token.is_empty() {
            return Some((false, token));
        }
    }
    None
}

fn accepts_html(accept: &str) -> bool {
    accept.is_empty() || accept.contains("*/*") || accept.contains("text/html")
}

fn render_tags(index: &str, title: &str, description: &str, image_url: &str) -> String {
    let title = escape_html(title);
    let description = escape_html(description);
    let image_url = escape_html(image_url);
    let image = if image_url.is_empty() {
        String::new()
    } else {
        format!("\n    <meta property=\"og:image\" content=\"{image_url}\" />")
    };
    let tags = format!(
        "
    <meta name=\"description\" content=\"{description}\" />

    <!-- Facebook Meta Tags -->
    <meta property=\"og:type\" content=\"website\" />
    <meta property=\"og:title\" content=\"{title}\" />
    <meta property=\"og:description\" content=\"{description}\" />{image}"
    );
    index.replace("<!-- metadata:tags -->", &tags)
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn origin(request: &Request) -> String {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let https = request.uri().scheme_str() == Some("https")
        || request
            .headers()
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .next()
                    .is_some_and(|item| item.trim() == "https")
            });
    let scheme = if https { "https" } else { "http" };
    format!("{scheme}://{host}")
}

async fn static_file(app: &App, path: &str) -> Option<Response> {
    if path.contains("..") || path.starts_with("/api") || path == "/" {
        return None;
    }
    let relative = path.trim_start_matches('/');
    if relative.is_empty() {
        return None;
    }
    let file = app.web_root().join(relative);
    if !file.is_file() {
        return None;
    }
    let bytes = tokio::fs::read(&file).await.ok()?;
    let content_type = content_type(relative);
    Some(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
