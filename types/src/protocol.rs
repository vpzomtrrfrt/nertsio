use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HandshakeMessageC2S {
    Hello { name: String, game_id: u32 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HandshakeMessageS2C {
    Hello,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum GameMessageC2S {
    UpdateSelfReady { value: bool },
    ApplyHandAction { action: crate::HandAction },
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
}
