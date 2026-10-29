//! `GET /ws/lobby/{code}?name=...` — the realtime half of a lobby: join, see the player list
//! update live, ready up, and (the host only, once everyone is ready) start the match. One
//! socket per player; the socket closing for any reason — tab closed, network drop, explicit
//! leave — removes that player, and the lobby itself is dropped once it's empty.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use survivors_lobby_protocol::{ClientMsg, ServerMsg};

use crate::state::{AppState, Player};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws/lobby/{code}", get(join_lobby))
        .with_state(state)
}

#[derive(Deserialize)]
struct JoinQuery {
    name: String,
}

async fn join_lobby(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<JoinQuery>,
) -> impl IntoResponse {
    let code = code.to_uppercase();
    ws.on_upgrade(move |socket| handle_socket(socket, state, code, q.name))
}

async fn handle_socket(socket: WebSocket, state: AppState, code: String, name: String) {
    let (mut sink, mut stream) = socket.split();

    // Register the player and grab what this connection needs before anything else runs:
    // its own id, a subscription to the lobby's broadcast channel, and a snapshot of the
    // current state to open with.
    let registered = {
        let mut lobbies = state.lobbies.write().await;
        let Some(lobby) = lobbies.get_mut(&code) else {
            let _ = sink
                .send(ws_msg(&ServerMsg::Error {
                    message: format!("no lobby with code {code}"),
                }))
                .await;
            return;
        };
        if lobby.players.len() >= lobby.max_players as usize {
            let _ = sink
                .send(ws_msg(&ServerMsg::Error {
                    message: "lobby is full".into(),
                }))
                .await;
            return;
        }
        let id = lobby.next_player_id;
        lobby.next_player_id += 1;
        let (direct_tx, direct_rx) = tokio::sync::mpsc::unbounded_channel();
        lobby.players.push(Player {
            id,
            name: name.clone(),
            ready: false,
            direct: direct_tx,
        });
        let rx = lobby.tx.subscribe();
        let welcome = ServerMsg::Welcome {
            you: id,
            state: lobby.state(&code),
        };
        lobby.broadcast_state(&code);
        tracing::info!(%code, player_id = id, %name, "player joined");
        (id, rx, direct_rx, welcome)
    };
    let (player_id, mut rx, mut direct_rx, welcome) = registered;

    if sink.send(ws_msg(&welcome)).await.is_err() {
        cleanup(&state, &code, player_id).await;
        return;
    }

    // Forward both this lobby's shared broadcast (everyone) and this player's own direct
    // channel (replies meant for them alone) out over the one socket.
    let mut send_task = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                m = rx.recv() => match m {
                    Ok(m) => m,
                    Err(_) => break,
                },
                m = direct_rx.recv() => match m {
                    Some(m) => m,
                    None => break,
                },
            };
            if sink.send(ws_msg(&msg)).await.is_err() {
                break;
            }
        }
    });

    // Read client messages and act on them, until the socket closes.
    let state_for_recv = state.clone();
    let code_for_recv = code.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            let Message::Text(text) = msg else { continue };
            if let Ok(client_msg) = serde_json::from_str::<ClientMsg>(&text) {
                handle_client_msg(&state_for_recv, &code_for_recv, player_id, client_msg).await;
            }
        }
    });

    // Whichever task ends first (socket closed either direction) tears down the other.
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    cleanup(&state, &code, player_id).await;
}

async fn handle_client_msg(state: &AppState, code: &str, player_id: u32, msg: ClientMsg) {
    let mut lobbies = state.lobbies.write().await;
    let Some(lobby) = lobbies.get_mut(code) else {
        return;
    };
    match msg {
        ClientMsg::SetReady { ready } => {
            if let Some(p) = lobby.players.iter_mut().find(|p| p.id == player_id) {
                p.ready = ready;
            }
            lobby.broadcast_state(code);
        }
        ClientMsg::StartMatch => {
            if !lobby.is_host(player_id) {
                lobby.tell(
                    player_id,
                    ServerMsg::Error {
                        message: "only the host can start the match".into(),
                    },
                );
                return;
            }
            if !lobby.all_ready() {
                lobby.tell(
                    player_id,
                    ServerMsg::Error {
                        message: "not everyone is ready".into(),
                    },
                );
                return;
            }
            tracing::info!(%code, "match starting");
            let _ = lobby.tx.send(ServerMsg::MatchStarting {
                seed: rand::random(),
                relay_token: uuid::Uuid::new_v4().to_string(),
                host_mode: lobby.host_mode,
            });
        }
    }
}

async fn cleanup(state: &AppState, code: &str, player_id: u32) {
    let mut lobbies = state.lobbies.write().await;
    let Some(lobby) = lobbies.get_mut(code) else {
        return;
    };
    lobby.players.retain(|p| p.id != player_id);
    tracing::info!(%code, player_id, "player left");
    if lobby.players.is_empty() {
        lobbies.remove(code);
        tracing::info!(%code, "lobby closed (empty)");
    } else {
        lobby.broadcast_state(code);
    }
}

fn ws_msg(msg: &ServerMsg) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap_or_default().into())
}
