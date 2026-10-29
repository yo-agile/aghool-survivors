//! `GET/PUT /saves/{id}` — the VPS-backed half of character-progress persistence for the web
//! build. The desktop build never touches this: its save lives in a real file beside the exe
//! (the client's `local_state.rs`), untouched by any of this.
//!
//! The id is the save's only credential: a player who loses it loses the save, the same trust
//! model as an unlisted video link. The *client* mints it (a random UUID, generated once and
//! kept in the browser), never this server — this server only ever sees an id it's handed and
//! has no notion of which player, if any, holds it.
//!
//! One file per save, named by its id, under `--save-dir`. No database: a save is a few KB of
//! JSON, and a filesystem already gives atomic replace and trivial backup (`rsync` the dir).

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::extract::{Path as AxPath, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

/// A save is a few KB of JSON; this is generous headroom, not a real limit on anything.
const MAX_SAVE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct SaveStore {
    dir: PathBuf,
}

impl SaveStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

pub fn router(store: SaveStore) -> Router {
    Router::new()
        .route("/saves/{id}", get(get_save).put(put_save))
        .with_state(store)
}

/// A save id as the client must mint it: a UUID, nothing else. Rejecting anything that isn't one
/// keeps `id` from ever reaching the filesystem as a path component unvalidated — no `../`, no
/// absolute paths, nothing but a hyphenated hex string.
fn save_path(dir: &Path, id: &str) -> Option<PathBuf> {
    uuid::Uuid::parse_str(id).ok()?;
    Some(dir.join(format!("{id}.json")))
}

async fn get_save(
    State(store): State<SaveStore>,
    AxPath(id): AxPath<String>,
) -> impl IntoResponse {
    let Some(path) = save_path(&store.dir, &id) else {
        return (StatusCode::BAD_REQUEST, "id must be a uuid").into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "no save with this id yet").into_response()
        }
        Err(e) => {
            tracing::error!(%id, error = %e, "save read failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "read failed").into_response()
        }
    }
}

async fn put_save(
    State(store): State<SaveStore>,
    AxPath(id): AxPath<String>,
    body: Bytes,
) -> impl IntoResponse {
    let Some(path) = save_path(&store.dir, &id) else {
        return (StatusCode::BAD_REQUEST, "id must be a uuid").into_response();
    };
    if body.len() > MAX_SAVE_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "save too large").into_response();
    }
    if let Err(e) = write_atomic(&path, &body).await {
        tracing::error!(%id, error = %e, "save write failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response();
    }
    tracing::info!(%id, bytes = body.len(), "save written");
    StatusCode::NO_CONTENT.into_response()
}

/// Write through a temp file and rename, so a crash mid-write never corrupts the last good save
/// — the same shape as the client's own `local_state::write_atomic`.
async fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, contents).await?;
    tokio::fs::rename(&tmp, path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_uuid_resolves_to_a_path() {
        let dir = PathBuf::from("/data/saves");
        assert!(save_path(&dir, "not-a-uuid").is_none());
        assert!(save_path(&dir, "../../etc/passwd").is_none());
        assert!(save_path(&dir, "").is_none());
        let id = uuid::Uuid::new_v4().to_string();
        let path = save_path(&dir, &id).unwrap();
        assert_eq!(path, dir.join(format!("{id}.json")));
    }
}
