use crate::{config::DownloadSettings, model::AddRequest, queue::Queue, resolver};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use std::sync::Arc;

type Error = (StatusCode, Json<serde_json::Value>);
fn error(e: impl std::fmt::Display) -> Error {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":e.to_string()})),
    )
}
pub fn router(q: Arc<Queue>) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/import.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/import.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .nest(
            "/api",
            Router::new()
                .route("/tasks", get(list).post(add))
                .route("/tasks/{id}", axum::routing::delete(delete))
                .route("/tasks/{id}/cancel", post(cancel))
                .route("/tasks/{id}/retry", post(retry))
                .route("/tasks/{id}/log", get(log))
                .route("/info", get(info))
                .route("/settings", get(settings).put(save_settings))
                .route_layer(middleware::from_fn_with_state(q.clone(), auth)),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
        .with_state(q)
}
async fn auth(State(q): State<Arc<Queue>>, req: Request, next: Next) -> Response {
    if let Some(token) = &q.config.token {
        let supplied = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "));
        if supplied != Some(token) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Enter a valid access token"})),
            )
                .into_response();
        }
    }
    // 禁止跨站表单/脚本发起状态变更。网页和 API 在同一个服务上。
    if req.method() != axum::http::Method::GET
        && let Some(origin) = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
    {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if url::Url::parse(origin).ok().and_then(|u| {
            u.host_str().map(|h| {
                format!(
                    "{}{}",
                    h,
                    u.port().map(|p| format!(":{p}")).unwrap_or_default()
                )
            })
        }) != Some(host.to_string())
        {
            return (StatusCode::FORBIDDEN, "Origin mismatch").into_response();
        }
    }
    next.run(req).await
}
async fn list(State(q): State<Arc<Queue>>) -> Result<impl IntoResponse, Error> {
    Ok(Json(q.store.list().map_err(error)?))
}
async fn info(State(q): State<Arc<Queue>>) -> Json<serde_json::Value> {
    let config = q.current_config();
    Json(
        json!({"version":env!("CARGO_PKG_VERSION"),"output":config.output,"workers":config.workers,"threads":config.threads,"retries":config.retries,"browser":resolver::browser::executable(&config).is_some(),"preview":config.preview_segments}),
    )
}
async fn settings(State(q): State<Arc<Queue>>) -> Json<DownloadSettings> {
    Json(q.current_config().download_settings())
}
async fn save_settings(
    State(q): State<Arc<Queue>>,
    Json(settings): Json<DownloadSettings>,
) -> Result<Json<DownloadSettings>, Error> {
    let saved = tokio::task::spawn_blocking(move || q.save_settings(settings))
        .await
        .map_err(error)?
        .map_err(error)?;
    Ok(Json(saved))
}
async fn add(
    State(q): State<Arc<Queue>>,
    Json(req): Json<AddRequest>,
) -> Result<impl IntoResponse, Error> {
    let headers = resolver::normalize_headers(req.headers).map_err(error)?;
    let urls = req
        .urls
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .collect::<Vec<_>>();
    if urls.is_empty() || urls.len() > 200 {
        return Err(error("Submit 1–200 URLs, one per line"));
    }
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    let mut rejected = Vec::new();
    for u in urls {
        match q.store.add(u, &headers) {
            Ok(Some(id)) => added.push(id),
            Ok(None) => skipped.push(u.to_owned()),
            Err(e) => rejected.push(json!({"url":u,"reason":e.to_string()})),
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({"added":added,"skipped":skipped,"rejected":rejected})),
    ))
}
async fn cancel(
    State(q): State<Arc<Queue>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, Error> {
    if !q.cancel(id).map_err(error)? {
        return Err(error("Task not found or already finished"));
    }
    Ok(Json(json!({"ok":true})))
}
async fn retry(
    State(q): State<Arc<Queue>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, Error> {
    if !q.retry(id).map_err(error)? {
        return Err(error("Only failed or cancelled tasks can be retried"));
    }
    Ok(Json(json!({"ok":true})))
}
async fn delete(
    State(q): State<Arc<Queue>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, Error> {
    if !q.delete(id).map_err(error)? {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"Task record not found"})),
        ));
    }
    Ok(Json(json!({"ok":true})))
}
async fn log(State(q): State<Arc<Queue>>, Path(id): Path<i64>) -> Result<impl IntoResponse, Error> {
    if q.store.get(id).map_err(error)?.is_none() {
        return Err(error("Task not found"));
    }
    let text = tokio::fs::read(crate::downloader::log_path(&q.config, id))
        .await
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_else(|_| "No logs yet".into());
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(text))
        .unwrap())
}
