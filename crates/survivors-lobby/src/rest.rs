//! `GET/POST /lobbies` — browsing and creating lobbies. The realtime half (joining, ready-up,
//! starting the match) is `ws.rs`; these are the two calls a lobby browser screen needs before
//! it ever opens a socket.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use survivors_lobby_protocol::{CreateLobbyRequest, CreateLobbyResponse, LobbySummary};

use crate::state::{generate_code, AppState, Lobby};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/lobbies", get(list_lobbies).post(create_lobby))
        .route("/lobbies/{code}", get(get_lobby))
        .with_state(state)
}

async fn create_lobby(
    State(state): State<AppState>,
    Json(req): Json<CreateLobbyRequest>,
) -> impl IntoResponse {
    if req.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "name must not be empty").into_response();
    }
    if req.max_players == 0 || req.max_players > 8 {
        return (StatusCode::BAD_REQUEST, "max_players must be 1..=8").into_response();
    }
    let mut lobbies = state.lobbies.write().await;
    let code = generate_code(&lobbies);
    lobbies.insert(
        code.clone(),
        Lobby::new(req.name, req.public, req.max_players, req.map, req.host_mode),
    );
    tracing::info!(%code, "lobby created");
    Json(CreateLobbyResponse { code }).into_response()
}

async fn list_lobbies(State(state): State<AppState>) -> Json<Vec<LobbySummary>> {
    let lobbies = state.lobbies.read().await;
    Json(
        lobbies
            .iter()
            .filter(|(_, l)| l.public)
            .map(|(code, l)| l.summary(code))
            .collect(),
    )
}

/// Lets a client holding a (possibly private) code preflight it — confirm it exists, see its
/// name/map/player count — before opening the WebSocket to actually join.
async fn get_lobby(State(state): State<AppState>, Path(code): Path<String>) -> impl IntoResponse {
    let code = code.to_uppercase();
    let lobbies = state.lobbies.read().await;
    match lobbies.get(&code) {
        Some(l) => Json(l.summary(&code)).into_response(),
        None => (StatusCode::NOT_FOUND, "no such lobby").into_response(),
    }
}
