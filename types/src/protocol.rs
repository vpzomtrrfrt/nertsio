use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub enum HandshakeMessageC2S {
    Hello { name: String, game_id: u32 },
}

#[derive(Deserialize, Serialize)]
pub enum HandshakeMessageS2C {
    Joined {
        info: crate::GameState,
        your_player_id: u8,
    },
}

#[derive(Deserialize, Serialize)]
pub enum GameMessageC2S {
    UpdateSelfReady { value: bool },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum GameMessageS2C {
    PlayerJoin {
        id: u8,
        info: crate::GamePlayerState,
    },
    PlayerUpdateReady {
        id: u8,
        value: bool,
    },
}
