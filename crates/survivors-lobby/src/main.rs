//! `survivors-lobby` — standalone matchmaking + save server for Warcraft Survivors co-op:
//! players create or join public/private lobbies by a short join code, ready up, and the host
//! (or, once `HostMode::DedicatedServer` is wired up, a separate headless process) triggers the
//! handoff into a run. It also holds the web build's character-progress saves, since the desktop
//! build already has a real filesystem to save to and the browser build doesn't.
//!
//! This binary knows nothing about the game client, the realm/world protocol, or game data — it
//! only brokers lobbies and stores opaque save blobs by an id the client itself mints. Unlike
//! `wenilla-host` (which hands out the operator's MPQ files unauthenticated) it carries nothing
//! sensitive by default, so it's meant to be bound to a public address and reached directly from
//! the open internet.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::http::Method;
use clap::Parser;
use survivors_lobby::{rest, save, state, ws};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

#[derive(Parser)]
#[command(about = "Matchmaking + save server for Warcraft Survivors co-op")]
struct Cli {
    /// Address to listen on. Defaults to every interface: lobbies carry no game data, and saves
    /// are opaque blobs keyed by a client-minted id, so (unlike wenilla-host) there's nothing
    /// unsafe about exposing this directly.
    #[arg(long, default_value = "0.0.0.0:8099")]
    bind: String,
    /// Where web-build character saves are written, one file per save id. Created if missing.
    #[arg(long, default_value = "./saves")]
    save_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    let lobby_state = state::AppState::default();
    let save_store = save::SaveStore::new(cli.save_dir.clone());
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT])
        .allow_headers(Any);

    let app = rest::router(lobby_state.clone())
        .merge(ws::router(lobby_state))
        .merge(save::router(save_store))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    let listener = tokio::net::TcpListener::bind(&cli.bind)
        .await
        .with_context(|| format!("binding {}", cli.bind))?;
    tracing::info!(
        bind = %cli.bind,
        save_dir = %cli.save_dir.display(),
        "survivors-lobby listening"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("serving")
}
