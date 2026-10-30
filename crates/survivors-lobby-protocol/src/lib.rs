//! Wire types for the Warcraft Survivors co-op lobby: the REST shapes for browsing/creating
//! lobbies, and the WebSocket messages exchanged once a client joins one (`/ws/lobby/{code}`).
//!
//! This crate is deliberately thin — `serde` only, no transport — so a Bevy client can depend on
//! it unchanged on both the native and wasm32 targets, the same way `benilla-protocol` keeps its
//! message types free of the transport seam that carries them.

use serde::{Deserialize, Serialize};

/// A short, human-typeable code a lobby is joined by, e.g. `"QX7K2M"`. Not case sensitive;
/// servers normalize to uppercase.
pub type LobbyCode = String;

/// Fewest and most players one lobby may hold. These are the server's *admission* limits — a
/// design cap, not a measured one: how many players a run can actually carry depends on the
/// simulation cost of the match relay, which doesn't exist yet. Raise `MAX_PLAYERS` only after
/// measuring that.
pub const MIN_PLAYERS: u8 = 2;
pub const MAX_PLAYERS: u8 = 8;
/// What a new lobby offers by default; the lobby page preselects it.
pub const DEFAULT_PLAYERS: u8 = 4;

/// Longest display name (characters) for a player, a lobby, and a zone label. Servers trim and
/// truncate to these rather than reject, so a long paste never strands a player at the form.
pub const MAX_PLAYER_NAME_CHARS: usize = 24;
pub const MAX_LOBBY_NAME_CHARS: usize = 32;
pub const MAX_ZONE_NAME_CHARS: usize = 40;

/// The nine classes a player can bring, spelled exactly as the game's class table
/// (`benilla-app/src/survivors/data.rs`) spells them. The server accepts nothing else in
/// `PlayerInfo::class`, so a roster never shows a string a client invented.
pub const CLASSES: [&str; 9] = [
    "Warrior", "Paladin", "Hunter", "Rogue", "Priest", "Shaman", "Mage", "Warlock", "Druid",
];

/// Who simulates enemies, loot and damage for a running match. The lobby and its wire protocol
/// don't care which: both are "one seat broadcasts authoritative state, everyone else sends
/// input," just occupied by different things.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostMode {
    /// The lobby's host player's own client runs the simulation.
    PlayerHosted,
    /// A separate, headless process (run by the operator, not a player) runs the simulation.
    /// Reserved: the server refuses to create such a lobby until that process exists.
    DedicatedServer,
}

/// What a lobby browser screen shows for one public lobby, without joining it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LobbySummary {
    pub code: LobbyCode,
    pub name: String,
    pub players: u8,
    pub max_players: u8,
    pub map: String,
    pub host_mode: HostMode,
}

/// `POST /lobbies`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateLobbyRequest {
    pub name: String,
    /// Public lobbies show up in `GET /lobbies`; private ones are reachable only by code.
    pub public: bool,
    pub max_players: u8,
    pub map: String,
    pub host_mode: HostMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateLobbyResponse {
    pub code: LobbyCode,
}

/// One connected player, as the lobby roster shows them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    /// Unique within this lobby's lifetime; not a reusable account id.
    pub id: u32,
    pub name: String,
    /// One of [`CLASSES`], or empty while the player hasn't picked yet.
    #[serde(default)]
    pub class: String,
    pub ready: bool,
    /// The first player to join is the host: they pick ready-up timing and press start. Ignored
    /// when `host_mode` is `DedicatedServer`, where no player seat is authoritative.
    pub is_host: bool,
}

/// The full lobby roster and settings, sent on join and after every change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LobbyState {
    pub code: LobbyCode,
    pub name: String,
    pub map: String,
    pub max_players: u8,
    pub host_mode: HostMode,
    pub players: Vec<PlayerInfo>,
}

/// Client → server, over `/ws/lobby/{code}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    /// Toggle this player's ready state.
    SetReady { ready: bool },
    /// Change this player's class (one of [`CLASSES`]). Clears their ready state, so nobody is
    /// counted ready for a build they've since changed.
    SetClass { class: String },
    /// Host only, and only once everyone is ready: begin the handoff into a match.
    StartMatch,
}

/// Server → client, over `/ws/lobby/{code}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    /// The first message on a successful join: this player's own id plus the current state.
    Welcome { you: u32, state: LobbyState },
    /// Sent to everyone whenever the roster or ready state changes.
    State(LobbyState),
    /// The match is starting. `relay_token` is what the client hands to the (not yet built)
    /// match relay to authenticate into this specific run; `seed` keeps the run deterministic
    /// across clients wherever the sim needs one.
    MatchStarting {
        seed: u64,
        relay_token: String,
        host_mode: HostMode,
    },
    /// A rejected action (wrong lobby, not host, not everyone ready, lobby full, ...), with a
    /// message fit to show the player directly.
    Error { message: String },
}
