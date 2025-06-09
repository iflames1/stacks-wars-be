use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use crate::ws::rules::RuleContext;

#[derive(Debug)]
pub struct GameRoom {
    pub info: GameRoomInfo,
    pub players: Vec<RoomPlayer>,
    pub rankings: Vec<(Uuid, usize)>,
    pub used_words: HashSet<String>,

    pub current_turn_id: Uuid,
    pub rule_context: RuleContext,
    pub rule_index: usize,
    pub game_over: bool,
    pub eliminated_players: Vec<RoomPlayer>,
}

#[derive(Serialize)]
pub struct Standing {
    pub wallet_address: String,
    pub rank: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub wallet_address: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    NotReady,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomPlayer {
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
    pub creator_id: Uuid,
    pub max_participants: usize,
    pub state: GameState,
}

#[derive(Deserialize)]
pub struct QueryParams {
    pub player_id: Uuid,
}
