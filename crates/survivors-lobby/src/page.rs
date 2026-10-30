//! `GET /` and `GET /lobby` — the lobby page itself, compiled into the binary so the server stays
//! one file to copy to a VPS. The page talks to this same server (`/lobbies`, `/ws/lobby/{code}`)
//! on its own origin, so there is nothing to configure behind a reverse proxy: it picks `wss:`
//! when it was loaded over `https:`.

use axum::http::header;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;

const LOBBY_HTML: &str = include_str!("../assets/lobby.html");

/// The page is one self-contained file with inline script and style, and every piece of text it
/// shows from another player goes in through `textContent`, never markup. The policy keeps it
/// that way: no external scripts, styles, frames or form posts, and it may only talk to the
/// origin it came from (or a `?server=` the player chose, over `http(s):`/`ws(s):`).
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                   img-src data:; connect-src 'self' http: https: ws: wss:; base-uri 'none'; \
                   form-action 'none'; frame-ancestors 'none'";

pub fn router() -> Router {
    Router::new()
        .route("/", get(page))
        .route("/lobby", get(page))
}

async fn page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_SECURITY_POLICY, CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Html(LOBBY_HTML),
    )
}
