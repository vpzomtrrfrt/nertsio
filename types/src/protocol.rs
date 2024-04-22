use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub const COORDINATOR_CHANNEL: &str = "gameserver_states";
pub const PROTOCOL_VERSION: u16 = 7;

pub const CLOSE_KICK: u8 = 1;
pub const CLOSE_TOO_OLD: u8 = 2;
pub const CLOSE_TOO_NEW: u8 = 3;

pub fn get_close_message(code: u8) -> &'static str {
    match code {
        CLOSE_KICK => "Kicked",
        CLOSE_TOO_OLD => "Version Too Old",
        CLOSE_TOO_NEW => "Version Too New",
        _ => "Unknown Close Reason",
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum MaintenanceMessageC2S {
    Hello {
        name: String,
        game_id: Option<u32>,
        new_game_public: Option<bool>,
        protocol_version: u16,
        min_protocol_version: u16,
    },
    Ping,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum MaintenanceMessageS2C {
    Hello,
    Pong,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum GameMessageC2S {
    UpdateSelfReady { value: bool },
    ForceStart,
    ApplyHandAction { action: crate::HandAction },
    CallNerts,
    AddBot,
    KickPlayer { player: u8 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum GameMessageS2C {
    Joined {
        info: crate::GameState,
        your_player_id: u8,
    },
    PlayerJoin {
        id: u8,
        info: crate::GamePlayerState,
    },
    PlayerLeave {
        id: u8,
    },
    PlayerUpdateReady {
        id: u8,
        value: bool,
    },
    HandInit {
        info: crate::HandState,
        delay: std::time::Duration,
    },
    PlayerHandAction {
        player: u8,
        action: crate::HandAction,
    },
    NertsCalled {
        player: u8,
    },
    HandEnd {
        scores: Vec<i32>,
    },
    HandStalled,
    HandStallCancel,
    ServerHandAction {
        action: crate::HandAction,
    },
    GameEnd,
    NewMasterPlayer {
        player: u8,
    },
    HandStart,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum DatagramMessageC2S {
    UpdateMouseState { seq: u32, state: crate::MouseState },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum DatagramMessageS2C {
    UpdateMouseState {
        player_idx: u8,
        seq: u32,
        state: crate::MouseState,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublicGameInfo {
    pub game_id: u32,
    pub players: u8,
    pub waiting: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerStatusMessage<'a> {
    pub server_id: u8,
    pub protocol_version: u16,
    pub min_protocol_version: u16,
    pub address_ipv4: std::net::SocketAddrV4,
    pub open_public_games: Vec<PublicGameInfo>,
    pub stats: ServerStats,
    pub hostname: Option<Cow<'a, str>>,
    pub web_port: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConnectionInfo<'a> {
    pub server_id: u8,
    pub address_ipv4: std::net::SocketAddrV4,
    pub hostname: Option<Cow<'a, str>>,
    pub web_port: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RespList<T> {
    pub items: Vec<T>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublicGameInfoExpanded<'a> {
    pub game_id: u32,
    pub players: u8,
    pub waiting: bool,
    pub server: ServerConnectionInfo<'a>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerStats {
    pub public_games: u32,
    pub private_games: u32,
    pub public_game_players: u32,
    pub private_game_players: u32,
}
