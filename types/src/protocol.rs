use serde::{Deserialize, Serialize};

pub const COORDINATOR_CHANNEL: &str = "gameserver_states";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HandshakeMessageC2S {
    Hello {
        name: String,
        game_id: Option<u32>,
        new_game_public: Option<bool>,
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
    pub address_ipv4: std::net::SocketAddrV4,
    pub open_public_games: Vec<PublicGameInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConnectionInfo {
    pub server_id: u8,
    pub address_ipv4: std::net::SocketAddrV4,
}
