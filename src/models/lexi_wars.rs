use crate::models::game::Player;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LexiWarsClientMessage {
    WordEntry { word: String },
    Ping { ts: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerStanding {
    pub player: Player,
    pub rank: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LexiWarsServerMessage {
    Turn { current_turn: Player },
    Rule { rule: String },
    Countdown { time: u64 },
    Rank { rank: String },
    Validate { msg: String },
    WordEntry { word: String, sender: Player },
    UsedWord { word: String },
    GameOver,
    FinalStanding { standing: Vec<PlayerStanding> },
    Prize { amount: f64 },
    Pong { ts: u64, pong: u64 },
    Start { time: u32, started: bool },
    StartFailed,
    AlreadyStarted,
}
