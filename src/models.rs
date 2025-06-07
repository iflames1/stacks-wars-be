use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use crate::ws::rules::RuleContext;

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
}

#[derive(Debug, Clone)]
pub struct Player {
    pub id: Uuid,
    pub username: String,
}

#[derive(Deserialize)]
pub struct QueryParams {
    pub player_id: Uuid,
}

#[derive(Debug)]
pub struct GameRoom {
    pub id: Uuid,
    pub players: Vec<Player>,
    pub eliminated_players: Vec<Player>,
    pub current_turn_id: Uuid,
    pub used_words: HashSet<String>,
    pub rule_context: RuleContext,
    pub rule_index: usize,
    pub game_over: bool,
    pub rankings: Vec<(Uuid, usize)>,
}

#[derive(Serialize)]
pub struct Standing {
    pub username: String,
    pub rank: usize,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GameState {
    Waiting,
    InProgress,
    Finished,
}

#[derive(Serialize, Deserialize)]
pub struct GameRoomInfo {
    pub id: Uuid,
    pub name: String,
    pub creator_id: Uuid,
    pub max_participants: usize,
    pub state: GameState,
}
