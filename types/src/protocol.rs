use serde::{Deserialize, Serialize};

pub const COORDINATOR_CHANNEL: &str = "gameserver_states";
pub const PROTOCOL_VERSION: u16 = 4;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HandshakeMessageC2S {
    Hello {
        name: String,
        game_id: Option<u32>,
        new_game_public: Option<bool>,
        protocol_version: u16,
        min_protocol_version: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HandshakeMessageS2C {
    Hello,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum GameMessageC2S {
    UpdateSelfReady { value: bool },
    ApplyHandAction { action: crate::HandAction },
    CallNerts,
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
    HandStart {
        info: crate::HandState,
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
pub struct ServerStatusMessage {
    pub server_id: u8,
    pub protocol_version: u16,
    pub min_protocol_version: u16,
    pub address_ipv4: std::net::SocketAddrV4,
    pub open_public_games: Vec<PublicGameInfo>,
    pub stats: ServerStats,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConnectionInfo {
    pub server_id: u8,
    pub address_ipv4: std::net::SocketAddrV4,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RespList<T> {
    pub items: Vec<T>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublicGameInfoExpanded {
    pub game_id: u32,
    pub players: u8,
    pub waiting: bool,
    pub server: ServerConnectionInfo,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerStats {
    pub public_games: u32,
    pub private_games: u32,
    pub public_game_players: u32,
    pub private_game_players: u32,
}
