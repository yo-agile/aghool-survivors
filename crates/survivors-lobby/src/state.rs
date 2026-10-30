//! The in-memory lobby store. One process, no persistence: a lobby lives for as long as someone
//! is in it and is dropped the moment the last player leaves (or the process restarts) — same
//! lifetime as the WebSocket connections that make it up. A lobby nobody ever joins (created over
//! REST, tab closed before the socket opened) is swept after [`ABANDONED_AFTER`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng as _;
use survivors_lobby_protocol::{HostMode, LobbyState, PlayerInfo, ServerMsg};
use tokio::sync::{broadcast, mpsc, RwLock};

/// How long a lobby may sit with nobody in it before the sweeper drops it. Long enough to cover a
/// slow page load between `POST /lobbies` and the WebSocket join, short enough that abandoned
/// lobbies can't pile up on a server that's open to the internet.
pub const ABANDONED_AFTER: Duration = Duration::from_secs(120);

/// Upper bound on lobbies held at once; creation past it is refused. A guard against someone
/// scripting `POST /lobbies`, not a capacity plan.
pub const MAX_LOBBIES: usize = 500;

#[derive(Clone, Default)]
pub struct AppState {
    pub lobbies: Arc<RwLock<HashMap<String, Lobby>>>,
}

impl AppState {
    /// Drop every lobby that has had no players for longer than `ttl`. Returns how many went.
    pub async fn sweep_abandoned(&self, ttl: Duration) -> usize {
        let mut lobbies = self.lobbies.write().await;
        let before = lobbies.len();
        lobbies.retain(|_, l| !l.players.is_empty() || l.created.elapsed() < ttl);
        before - lobbies.len()
    }
}

pub struct Player {
    pub id: u32,
    pub name: String,
    /// One of `survivors_lobby_protocol::CLASSES`, or empty until picked.
    pub class: String,
    pub ready: bool,
    /// This player's own socket, for a reply only they should see (e.g. "you're not the host").
    /// Separate from `Lobby::tx`, which is for everyone.
    pub direct: mpsc::UnboundedSender<ServerMsg>,
}

pub struct Lobby {
    pub name: String,
    pub public: bool,
    pub max_players: u8,
    pub map: String,
    pub host_mode: HostMode,
    pub players: Vec<Player>,
    pub next_player_id: u32,
    pub created: Instant,
    /// Set once `MatchStarting` has gone out: the lobby stops admitting players and drops out of
    /// the public listing, since a run already under way can't take a late joiner.
    pub started: bool,
    /// Every connected socket subscribes to this; a fresh joiner gets the current state
    /// directly in its `Welcome` instead, so it never needs this channel's backlog.
    pub tx: broadcast::Sender<ServerMsg>,
}

impl Lobby {
    pub fn new(
        name: String,
        public: bool,
        max_players: u8,
        map: String,
        host_mode: HostMode,
    ) -> Self {
        let (tx, _rx) = broadcast::channel(32);
        Self {
            name,
            public,
            max_players,
            map,
            host_mode,
            players: Vec::new(),
            next_player_id: 0,
            created: Instant::now(),
            started: false,
            tx,
        }
    }

    pub fn state(&self, code: &str) -> LobbyState {
        LobbyState {
            code: code.to_string(),
            name: self.name.clone(),
            map: self.map.clone(),
            max_players: self.max_players,
            host_mode: self.host_mode,
            players: self
                .players
                .iter()
                .enumerate()
                .map(|(i, p)| PlayerInfo {
                    id: p.id,
                    name: p.name.clone(),
                    class: p.class.clone(),
                    ready: p.ready,
                    // Meaningless under DedicatedServer, where no player seat is authoritative;
                    // callers that care branch on `host_mode`, not on this flag, in that case.
                    is_host: i == 0,
                })
                .collect(),
        }
    }

    pub fn summary(&self, code: &str) -> survivors_lobby_protocol::LobbySummary {
        survivors_lobby_protocol::LobbySummary {
            code: code.to_string(),
            name: self.name.clone(),
            players: self.players.len() as u8,
            max_players: self.max_players,
            map: self.map.clone(),
            host_mode: self.host_mode,
        }
    }

    pub fn broadcast_state(&self, code: &str) {
        // Err only means no receiver is currently subscribed (e.g. the last player's send task
        // hasn't unsubscribed yet during teardown); nothing to do about it either way.
        let _ = self.tx.send(ServerMsg::State(self.state(code)));
    }

    pub fn is_host(&self, player_id: u32) -> bool {
        self.players.first().is_some_and(|p| p.id == player_id)
    }

    /// Reply to one player only — for rejections like "not the host," which the rest of the
    /// lobby has no reason to see.
    pub fn tell(&self, player_id: u32, msg: ServerMsg) {
        if let Some(p) = self.players.iter().find(|p| p.id == player_id) {
            let _ = p.direct.send(msg);
        }
    }

    pub fn all_ready(&self) -> bool {
        !self.players.is_empty() && self.players.iter().all(|p| p.ready)
    }
}

/// Trim, drop control characters, and truncate to `max_chars` characters. Everything a client
/// sends that other players will read goes through this, so the roster can never carry a
/// newline, an escape sequence, or a megabyte.
pub fn clean_text(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A 6-character code from an alphabet with the easily-confused characters (`0/O`, `1/I/L`)
/// removed, since players read these off a screen and type them into another one.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

pub fn generate_code(existing: &HashMap<String, Lobby>) -> String {
    let mut rng = rand::thread_rng();
    loop {
        let code: String = (0..6)
            .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
            .collect();
        if !existing.contains_key(&code) {
            return code;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_controls_trims_and_truncates() {
        assert_eq!(clean_text("  Arthas\n\t", 24), "Arthas");
        assert_eq!(clean_text("a\u{1b}[31mb", 24), "a[31mb");
        assert_eq!(clean_text("abcdefghij", 4), "abcd");
        assert_eq!(clean_text("ab  cd", 3), "ab");
        assert_eq!(clean_text("   ", 24), "");
        // Counts characters, not bytes: a multibyte name isn't cut mid-codepoint.
        assert_eq!(clean_text("ÅÅÅÅ", 2), "ÅÅ");
    }
}
