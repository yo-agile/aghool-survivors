//! `survivors-lobby` — standalone matchmaking server for Warcraft Survivors co-op: players
//! create or join public/private lobbies by a short join code, ready up, and the host (or, once
//! `HostMode::DedicatedServer` is wired up, a separate headless process) triggers the handoff
//! into a run.
//!
//! This binary knows nothing about the game client, the realm/world protocol, or game data — it
//! only brokers lobbies. Unlike `wenilla-host` (which hands out the operator's MPQ files
//! unauthenticated) it carries nothing sensitive, so it's meant to be bound to a public address
//! and reached directly from the open internet.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::http::Method;
use clap::Parser;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

mod rest;
mod state;
mod ws;

#[derive(Parser)]
#[command(about = "Matchmaking server for Warcraft Survivors co-op lobbies")]
struct Cli {
    /// Address to listen on. Defaults to every interface: this server holds no game data and no
    /// player credentials, so (unlike wenilla-host) there's nothing unsafe about exposing it.
    #[arg(long, default_value = "0.0.0.0:8099")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    let state = state::AppState::default();
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let app = rest::router(state.clone())
        .merge(ws::router(state))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    let listener = tokio::net::TcpListener::bind(&cli.bind)
        .await
        .with_context(|| format!("binding {}", cli.bind))?;
    tracing::info!(bind = %cli.bind, "survivors-lobby listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("serving")
}
