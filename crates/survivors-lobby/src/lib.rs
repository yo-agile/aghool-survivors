//! Library half of `survivors-lobby`, split out so its modules carry unit and integration tests
//! even though the `[[bin]]` itself runs with `test = false` — the same shape `wenilla-host` uses.

pub mod page;
pub mod rest;
pub mod save;
pub mod state;
pub mod ws;

use axum::Router;

/// Every route the server answers: the lobby page, lobby REST, the lobby WebSocket, and the
/// web-save store. `main` adds CORS and tracing on top; tests serve this as it stands.
pub fn router(lobbies: state::AppState, saves: save::SaveStore) -> Router {
    page::router()
        .merge(rest::router(lobbies.clone()))
        .merge(ws::router(lobbies))
        .merge(save::router(saves))
}
