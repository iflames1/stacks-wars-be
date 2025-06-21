use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::games::lexi_wars::rules::RuleContext;

#[derive(Debug)]
pub enum GameData {
    LexiWar {
        word_list: Arc<HashSet<String>>,
        // maybe future: round state, scores, difficulty, etc.
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameType {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub image_url: String,
    pub tags: Option<Vec<String>>,
    pub min_players: u8,
}

#[derive(Debug)]
pub struct GameRoom {
    pub info: GameRoomInfo,
    pub players: Vec<Player>,
    pub data: GameData,
    pub used_words_global: HashSet<String>,
    pub used_words: HashMap<Uuid, Vec<String>>,
    pub rule_context: RuleContext,
    pub rule_index: usize,

    pub current_turn_id: Uuid,
    pub eliminated_players: Vec<Player>,
}

#[derive(Serialize)]
pub struct Standing {
    pub wallet_address: String,
    pub rank: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    NotReady,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: Uuid,
    pub wallet_address: String,
    pub display_name: Option<String>,
    pub state: PlayerState,
    pub rank: Option<usize>,
    pub used_words: Vec<String>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum GameState {
    Waiting,
    InProgress,
    Finished,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GameRoomInfo {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub creator_id: Uuid,
    pub max_participants: usize,
    pub state: GameState,
    pub game_id: Uuid,
    pub game_name: String,
}
