//! `GET/POST /lobbies` — browsing and creating lobbies. The realtime half (joining, ready-up,
//! starting the match) is `ws.rs`; these are the calls a lobby browser screen needs before it
//! ever opens a socket.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use survivors_lobby_protocol::{
    CreateLobbyRequest, CreateLobbyResponse, HostMode, LobbySummary, MAX_LOBBY_NAME_CHARS,
    MAX_PLAYERS, MAX_ZONE_NAME_CHARS, MIN_PLAYERS,
};

use crate::state::{clean_text, generate_code, AppState, Lobby, MAX_LOBBIES};

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
    let name = clean_text(&req.name, MAX_LOBBY_NAME_CHARS);
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name must not be empty").into_response();
    }
    let map = clean_text(&req.map, MAX_ZONE_NAME_CHARS);
    if map.is_empty() {
        return (StatusCode::BAD_REQUEST, "pick a zone").into_response();
    }
    if !(MIN_PLAYERS..=MAX_PLAYERS).contains(&req.max_players) {
        return (
            StatusCode::BAD_REQUEST,
            format!("max_players must be {MIN_PLAYERS}..={MAX_PLAYERS}"),
        )
            .into_response();
    }
    if req.host_mode == HostMode::DedicatedServer {
        return (
            StatusCode::BAD_REQUEST,
            "dedicated-server lobbies aren't available yet; use player_hosted",
        )
            .into_response();
    }
    let mut lobbies = state.lobbies.write().await;
    if lobbies.len() >= MAX_LOBBIES {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the server is holding as many lobbies as it allows; try again shortly",
        )
            .into_response();
    }
    let code = generate_code(&lobbies);
    lobbies.insert(
        code.clone(),
        Lobby::new(name, req.public, req.max_players, map, req.host_mode),
    );
    tracing::info!(%code, "lobby created");
    Json(CreateLobbyResponse { code }).into_response()
}

async fn list_lobbies(State(state): State<AppState>) -> Json<Vec<LobbySummary>> {
    let lobbies = state.lobbies.read().await;
    let mut listed: Vec<_> = lobbies
        .iter()
        // A lobby with nobody in it yet is a creator mid-page-load, not somewhere to join; one
        // that has started can't take anyone.
        .filter(|(_, l)| l.public && !l.started && !l.players.is_empty())
        .map(|(code, l)| l.summary(code))
        .collect();
    // HashMap order is arbitrary and would make the list reshuffle on every refresh.
    listed.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.code.cmp(&b.code)));
    Json(listed)
}

/// Lets a client holding a (possibly private) code preflight it — confirm it exists, see its
/// name/map/player count — before opening the WebSocket to actually join.
async fn get_lobby(State(state): State<AppState>, Path(code): Path<String>) -> impl IntoResponse {
    let code = code.to_uppercase();
    let lobbies = state.lobbies.read().await;
    match lobbies.get(&code) {
        Some(l) if l.started => {
            (StatusCode::GONE, "that match has already started").into_response()
        }
        Some(l) => Json(l.summary(&code)).into_response(),
        None => (StatusCode::NOT_FOUND, "no such lobby").into_response(),
    }
}
