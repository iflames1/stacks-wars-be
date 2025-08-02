use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::games::lexi_wars::rules::RuleContext;

#[derive(Deserialize)]
pub struct WsQueryParams {
    pub user_id: Uuid,
}

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
    pub pool: Option<RoomPool>,
    pub connected_players: Vec<Player>,
    pub connected_players_count: usize,
    pub game_started: bool,
    pub current_rule: Option<String>,
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
#[serde(tag = "status", content = "data")]
pub enum ClaimState {
    Claimed { tx_id: String },
    NotClaimed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: Uuid,
    pub wallet_address: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    pub state: PlayerState,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_words: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<ClaimState>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub prize: Option<f64>,

    pub wars_point: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomPool {
    pub entry_amount: f64,
    pub contract_address: String,
    pub current_amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbyPoolInput {
    pub entry_amount: f64,
    pub contract_address: String,
    pub tx_id: String,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum GameState {
    Waiting,
    InProgress,
    Finished,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameRoomInfo {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub creator_id: Uuid,
    pub state: GameState,
    pub game_id: Uuid,
    pub game_name: String,
    pub participants: usize,
    pub contract_address: Option<String>,
    pub created_at: DateTime<Utc>,
    pub connected_players: Vec<Player>,
}

#[derive(Serialize)]
pub struct RoomExtended {
    pub info: GameRoomInfo,
    pub players: Vec<Player>,
    pub pool: Option<RoomPool>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum LobbyState {
    Waiting,
    InProgress,
    Finished,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LobbyInfo {
    pub id: Uuid,
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    pub creator_id: Uuid,
    pub state: LobbyState,
    pub game_id: Uuid,
    pub game_name: String,
    pub participants: usize,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,

    pub created_at: DateTime<Utc>,
    pub connected_players: Vec<Player>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LobbyPool {
    pub entry_amount: f64,
    pub contract_address: String,
    pub current_amount: f64,
}

#[derive(Serialize)]
pub struct LobbyExtended {
    pub info: LobbyInfo,
    pub players: Vec<Player>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<LobbyPool>,
}
