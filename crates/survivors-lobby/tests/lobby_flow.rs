//! The lobby server end to end: the real router, served on a real socket, driven by real
//! WebSocket clients. REST calls go through `oneshot` (no socket needed).

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use survivors_lobby::save::SaveStore;
use survivors_lobby::state::{AppState, Lobby, Player};
use survivors_lobby_protocol::{ClientMsg, HostMode, ServerMsg};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tower::ServiceExt;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(3);

struct Server {
    app: Router,
    addr: SocketAddr,
}

async fn start() -> Server {
    let state = AppState::default();
    let saves = SaveStore::new(
        std::env::temp_dir().join(format!("survivors-lobby-test-{}", uuid::Uuid::new_v4())),
    );
    let app = survivors_lobby::router(state.clone(), saves);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = app.clone();
    tokio::spawn(async move { axum::serve(listener, serve).await.unwrap() });
    Server { app, addr }
}

async fn request(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    request(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

async fn post_lobby(app: &Router, body: serde_json::Value) -> (StatusCode, String) {
    request(
        app,
        Request::post("/lobbies")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

fn lobby_body(name: &str, public: bool, max: u8) -> serde_json::Value {
    json!({ "name": name, "public": public, "max_players": max, "map": "Elwynn Forest", "host_mode": "player_hosted" })
}

async fn create(app: &Router, name: &str, public: bool, max: u8) -> String {
    let (status, body) = post_lobby(app, lobby_body(name, public, max)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str::<serde_json::Value>(&body).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn join(s: &Server, code: &str, name: &str, class: &str) -> Ws {
    let url = format!("ws://{}/ws/lobby/{code}?name={name}&class={class}", s.addr);
    connect_async(url).await.unwrap().0
}

async fn recv(ws: &mut Ws) -> ServerMsg {
    loop {
        let frame = tokio::time::timeout(WAIT, ws.next())
            .await
            .expect("timed out waiting for the server")
            .expect("socket closed")
            .expect("socket error");
        if let Message::Text(t) = frame {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

/// Read until a message satisfies `pick`; anything before it is skipped.
async fn recv_until<T>(ws: &mut Ws, mut pick: impl FnMut(&ServerMsg) -> Option<T>) -> T {
    loop {
        let m = recv(ws).await;
        if let Some(v) = pick(&m) {
            return v;
        }
    }
}

async fn say(ws: &mut Ws, msg: ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn create_rejects_bad_input() {
    let s = start().await;
    for (why, body) in [
        ("one player", lobby_body("x", true, 1)),
        ("nine players", lobby_body("x", true, 9)),
        ("blank name", lobby_body("   ", true, 4)),
        (
            "no zone",
            json!({ "name": "x", "public": true, "max_players": 4, "map": " ", "host_mode": "player_hosted" }),
        ),
        (
            "dedicated mode",
            json!({ "name": "x", "public": true, "max_players": 4, "map": "Elwynn Forest", "host_mode": "dedicated_server" }),
        ),
    ] {
        let (status, _) = post_lobby(&s.app, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}");
    }
    let (status, _) = post_lobby(&s.app, lobby_body("ok", true, 8)).await;
    assert_eq!(status, StatusCode::OK, "the cap itself is allowed");
}

#[tokio::test]
async fn names_are_cleaned_before_anyone_sees_them() {
    let s = start().await;
    let code = create(&s.app, &format!("  {}\n", "L".repeat(80)), true, 4).await;
    let mut ws = join(&s, &code, &format!("%20%0A{}", "N".repeat(60)), "Mage").await;
    let ServerMsg::Welcome { state, .. } = recv(&mut ws).await else {
        panic!("expected Welcome")
    };
    assert_eq!(state.name.chars().count(), 32);
    assert_eq!(state.players[0].name.chars().count(), 24);
    assert!(!state.players[0].name.contains('\n'));
}

#[tokio::test]
async fn only_joinable_public_lobbies_are_listed() {
    let s = start().await;
    let public = create(&s.app, "Open", true, 4).await;
    let private = create(&s.app, "Secret", false, 4).await;

    let (_, body) = get(&s.app, "/lobbies").await;
    assert_eq!(
        body, "[]",
        "a lobby nobody has joined yet is a creator mid-load, not a listing"
    );

    let _host = join(&s, &public, "Host", "Mage").await;
    let _other = join(&s, &private, "Other", "Rogue").await;
    // Joins race the listing; wait until the public one shows.
    let mut listed = String::new();
    for _ in 0..50 {
        listed = get(&s.app, "/lobbies").await.1;
        if listed.contains(&public) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(listed.contains(&public), "{listed}");
    assert!(
        !listed.contains(&private),
        "private lobbies never list: {listed}"
    );

    let (status, _) = get(&s.app, &format!("/lobbies/{}", private.to_lowercase())).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a private code still resolves, in any case"
    );
    let (status, _) = get(&s.app, "/lobbies/NOPE00").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn two_players_pick_classes_ready_up_and_start() {
    let s = start().await;
    let code = create(&s.app, "Duo", true, 4).await;

    let mut host = join(&s, &code, "Host", "Mage").await;
    let ServerMsg::Welcome {
        you: host_id,
        state,
    } = recv(&mut host).await
    else {
        panic!("expected Welcome")
    };
    assert!(state.players[0].is_host);
    assert_eq!(state.players[0].class, "Mage");
    assert_eq!(state.max_players, 4);

    // An unknown class at join time isn't fatal: the seat just has no class yet.
    let mut guest = join(&s, &code, "Guest", "Nonsense").await;
    let ServerMsg::Welcome {
        you: guest_id,
        state,
    } = recv(&mut guest).await
    else {
        panic!("expected Welcome")
    };
    assert_ne!(host_id, guest_id);
    assert_eq!(state.players.len(), 2);
    assert_eq!(state.players[1].class, "");

    say(&mut guest, ClientMsg::SetReady { ready: true }).await;
    say(
        &mut guest,
        ClientMsg::SetClass {
            class: "Druid".into(),
        },
    )
    .await;
    // Changing class cancels the ready that was given for the old build.
    let after = recv_until(&mut host, |m| match m {
        ServerMsg::State(st) if st.players.iter().any(|p| p.class == "Druid") => Some(st.clone()),
        _ => None,
    })
    .await;
    assert!(!after.players[1].ready, "class change must clear ready");

    say(
        &mut guest,
        ClientMsg::SetClass {
            class: "Gnome".into(),
        },
    )
    .await;
    let err = recv_until(&mut guest, |m| match m {
        ServerMsg::Error { message } => Some(message.clone()),
        _ => None,
    })
    .await;
    assert!(err.contains("unknown class"), "{err}");

    // Not everyone ready yet: even the host is refused, and the refusal is theirs alone.
    say(&mut host, ClientMsg::SetReady { ready: true }).await;
    say(&mut host, ClientMsg::StartMatch).await;
    let err = recv_until(&mut host, |m| match m {
        ServerMsg::Error { message } => Some(message.clone()),
        _ => None,
    })
    .await;
    assert!(err.contains("not everyone"), "{err}");

    say(&mut guest, ClientMsg::SetReady { ready: true }).await;
    say(&mut guest, ClientMsg::StartMatch).await;
    let err = recv_until(&mut guest, |m| match m {
        ServerMsg::Error { message } => Some(message.clone()),
        _ => None,
    })
    .await;
    assert!(err.contains("only the host"), "{err}");

    say(&mut host, ClientMsg::StartMatch).await;
    let pick = |m: &ServerMsg| match m {
        ServerMsg::MatchStarting {
            seed,
            relay_token,
            host_mode,
        } => Some((*seed, relay_token.clone(), *host_mode)),
        ServerMsg::Error { message } => {
            panic!("an error reached a player it wasn't for: {message}")
        }
        _ => None,
    };
    let a = recv_until(&mut host, pick).await;
    let b = recv_until(&mut guest, pick).await;
    assert_eq!(a, b, "everyone gets the same seed and token");
    assert_eq!(a.2, HostMode::PlayerHosted);

    // Started: closed to newcomers, gone from the list, and the preflight says why.
    let mut late = join(&s, &code, "Late", "Rogue").await;
    let ServerMsg::Error { message } = recv(&mut late).await else {
        panic!("expected Error")
    };
    assert!(message.contains("already started"), "{message}");
    assert_eq!(
        get(&s.app, &format!("/lobbies/{code}")).await.0,
        StatusCode::GONE
    );
    assert_eq!(get(&s.app, "/lobbies").await.1, "[]");
}

#[tokio::test]
async fn a_full_lobby_turns_people_away() {
    let s = start().await;
    let code = create(&s.app, "Tiny", true, 2).await;
    let mut a = join(&s, &code, "A", "Mage").await;
    let mut b = join(&s, &code, "B", "Mage").await;
    recv(&mut a).await;
    recv(&mut b).await;
    let mut c = join(&s, &code, "C", "Mage").await;
    let ServerMsg::Error { message } = recv(&mut c).await else {
        panic!("expected Error")
    };
    assert!(message.contains("full"), "{message}");
}

#[tokio::test]
async fn host_passes_to_the_next_player_and_empty_lobbies_close() {
    let s = start().await;
    let code = create(&s.app, "Handoff", true, 4).await;
    let mut a = join(&s, &code, "A", "Mage").await;
    let mut b = join(&s, &code, "B", "Rogue").await;
    recv(&mut a).await;
    recv(&mut b).await;
    drop(a);
    let st = recv_until(&mut b, |m| match m {
        ServerMsg::State(st) if st.players.len() == 1 => Some(st.clone()),
        _ => None,
    })
    .await;
    assert_eq!(st.players[0].name, "B");
    assert!(
        st.players[0].is_host,
        "the first player left, so the next one hosts"
    );
    drop(b);
    for _ in 0..50 {
        if get(&s.app, &format!("/lobbies/{code}")).await.0 == StatusCode::NOT_FOUND {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the lobby outlived its last player");
}

#[tokio::test]
async fn an_oversized_frame_drops_the_connection() {
    let s = start().await;
    let code = create(&s.app, "Big", true, 4).await;
    let mut ws = join(&s, &code, "A", "Mage").await;
    recv(&mut ws).await;
    let _ = ws.send(Message::Text("x".repeat(64 * 1024).into())).await;
    let closed = tokio::time::timeout(WAIT, async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert_eq!(closed, Ok(true), "a 64 KiB frame must not be accepted");
}

#[tokio::test]
async fn abandoned_lobbies_are_swept_but_occupied_ones_stay() {
    let state = AppState::default();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let mut lobbies = state.lobbies.write().await;
        lobbies.insert(
            "EMPTY2".into(),
            Lobby::new(
                "e".into(),
                true,
                4,
                "Elwynn Forest".into(),
                HostMode::PlayerHosted,
            ),
        );
        let mut busy = Lobby::new(
            "b".into(),
            true,
            4,
            "Elwynn Forest".into(),
            HostMode::PlayerHosted,
        );
        busy.players.push(Player {
            id: 0,
            name: "A".into(),
            class: String::new(),
            ready: false,
            direct: tx,
        });
        lobbies.insert("BUSY22".into(), busy);
    }
    assert_eq!(
        state.sweep_abandoned(Duration::from_secs(3600)).await,
        0,
        "too young to sweep"
    );
    assert_eq!(state.sweep_abandoned(Duration::ZERO).await, 1);
    let lobbies = state.lobbies.read().await;
    assert!(lobbies.contains_key("BUSY22") && !lobbies.contains_key("EMPTY2"));
}

#[tokio::test]
async fn the_lobby_page_is_served_with_a_policy() {
    let s = start().await;
    for path in ["/", "/lobby"] {
        let res = s
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"),
            "{csp}"
        );
        let body =
            String::from_utf8(to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
        assert!(body.contains("Co-op Lobby"), "the page itself");
    }
}
