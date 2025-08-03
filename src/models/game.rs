use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{errors::AppError, games::lexi_wars::rules::RuleContext};

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
    pub pool: Option<LobbyPool>,
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

impl FromStr for PlayerState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "notready" => Ok(PlayerState::NotReady),
            "ready" => Ok(PlayerState::Ready),
            other => Err(format!("Unknown PlayerState: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", content = "data")]
pub enum ClaimState {
    Claimed { tx_id: String },
    NotClaimed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: Uuid,
    pub wallet_address: String,
    pub state: PlayerState,
    pub wars_point: u64,

    pub username: Option<String>,
    pub display_name: Option<String>,
    pub rank: Option<usize>,
    pub used_words: Option<Vec<String>>,
    pub tx_id: Option<String>,
    pub claim: Option<ClaimState>,
    pub prize: Option<f64>,
}

impl Player {
    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("id".into(), self.id.to_string());
        map.insert("wallet_address".into(), self.wallet_address.clone());
        map.insert("state".into(), format!("{:?}", self.state));
        map.insert("wars_point".into(), self.wars_point.to_string());
        if let Some(ref username) = self.username {
            map.insert("username".into(), username.clone());
        }
        if let Some(ref display_name) = self.display_name {
            map.insert("display_name".into(), display_name.clone());
        }
        if let Some(ref used_words) = self.used_words {
            map.insert(
                "used_words".into(),
                serde_json::to_string(used_words).unwrap(),
            );
        }
        if let Some(rank) = self.rank {
            map.insert("rank".into(), rank.to_string());
        }
        if let Some(ref tx_id) = self.tx_id {
            map.insert("tx_id".into(), tx_id.clone());
        }
        if let Some(prize) = self.prize {
            map.insert("prize".into(), prize.to_string());
        }
        if let Some(ref claim) = self.claim {
            map.insert("claim".into(), serde_json::to_string(claim).unwrap());
        }
        map
    }

    pub fn from_redis_hash(map: &HashMap<String, String>) -> Result<Self, AppError> {
        Ok(Self {
            id: map
                .get("id")
                .ok_or_else(|| AppError::Deserialization("Missing id".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid UUID for id".into()))?,

            wallet_address: map
                .get("wallet_address")
                .ok_or_else(|| AppError::Deserialization("Missing wallet_address".into()))?
                .clone(),

            state: map
                .get("state")
                .ok_or_else(|| AppError::Deserialization("Missing state".into()))?
                .parse::<PlayerState>()
                .map_err(|_| AppError::Deserialization("Invalid PlayerState".into()))?,

            wars_point: map
                .get("wars_point")
                .ok_or_else(|| AppError::Deserialization("Missing wars_point".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid wars_point".into()))?,

            username: map.get("username").cloned(),
            display_name: map.get("display_name").cloned(),

            rank: map.get("rank").and_then(|s| s.parse().ok()),
            used_words: map
                .get("used_words")
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()),

            tx_id: map.get("tx_id").cloned(),

            claim: map
                .get("claim")
                .and_then(|s| serde_json::from_str::<ClaimState>(s).ok()),

            prize: map.get("prize").and_then(|s| s.parse().ok()),
        })
    }
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

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum LobbyState {
    Waiting,
    InProgress,
    Finished,
}

impl FromStr for LobbyState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Waiting" => Ok(LobbyState::Waiting),
            "InProgress" => Ok(LobbyState::InProgress),
            "Completed" => Ok(LobbyState::Finished),
            other => Err(format!("Unknown LobbyState: {}", other)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LobbyInfo {
    pub id: Uuid,
    pub name: String,

    pub creator_id: Uuid,
    pub state: LobbyState,
    pub game_id: Uuid,
    pub game_name: String,
    pub participants: usize,
    pub created_at: DateTime<Utc>,

    pub description: Option<String>,
    pub contract_address: Option<String>,
}

impl LobbyInfo {
    pub fn to_redis_hash(&self) -> Vec<(String, String)> {
        let mut fields = vec![
            ("id".into(), self.id.to_string()),
            ("name".into(), self.name.clone()),
            ("creator_id".into(), self.creator_id.to_string()),
            ("state".into(), format!("{:?}", self.state)),
            ("game_id".into(), self.game_id.to_string()),
            ("game_name".into(), self.game_name.clone()),
            ("participants".into(), self.participants.to_string()),
            ("created_at".into(), self.created_at.to_rfc3339()),
        ];
        if let Some(desc) = &self.description {
            fields.push(("description".into(), desc.clone()));
        }
        if let Some(addr) = &self.contract_address {
            fields.push(("contract_address".into(), addr.clone()));
        }
        fields
    }

    pub fn from_redis_hash(map: &HashMap<String, String>) -> Result<Self, AppError> {
        Ok(Self {
            id: map
                .get("id")
                .ok_or_else(|| AppError::Deserialization("Missing id".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid UUID for id".into()))?,

            name: map
                .get("name")
                .ok_or_else(|| AppError::Deserialization("Missing name".into()))?
                .clone(),

            creator_id: map
                .get("creator_id")
                .ok_or_else(|| AppError::Deserialization("Missing creator_id".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid UUID for creator_id".into()))?,

            state: map
                .get("state")
                .ok_or_else(|| AppError::Deserialization("Missing state".into()))?
                .parse::<LobbyState>()
                .map_err(|_| AppError::Deserialization("Invalid state".into()))?,

            game_id: map
                .get("game_id")
                .ok_or_else(|| AppError::Deserialization("Missing game_id".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid UUID for game_id".into()))?,

            game_name: map
                .get("game_name")
                .ok_or_else(|| AppError::Deserialization("Missing game_name".into()))?
                .clone(),

            participants: map
                .get("participants")
                .ok_or_else(|| AppError::Deserialization("Missing participants".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid participants count".into()))?,

            created_at: map
                .get("created_at")
                .ok_or_else(|| AppError::Deserialization("Missing created_at".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid datetime format".into()))?,

            description: map.get("description").cloned(),

            contract_address: map.get("contract_address").cloned(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LobbyPool {
    pub entry_amount: f64,
    pub contract_address: String,
    pub current_amount: f64,
}

impl LobbyPool {
    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("entry_amount".into(), self.entry_amount.to_string());
        map.insert("contract_address".into(), self.contract_address.clone());
        map.insert("current_amount".into(), self.current_amount.to_string());
        map
    }

    pub fn from_redis_hash(map: &HashMap<String, String>) -> Result<Self, AppError> {
        let entry_amount = map
            .get("entry_amount")
            .ok_or_else(|| AppError::Deserialization("Missing entry_amount".into()))?
            .parse()
            .map_err(|_| AppError::Deserialization("Invalid entry_amount".into()))?;
        let contract_address = map
            .get("contract_address")
            .ok_or_else(|| AppError::Deserialization("Missing contract_address".into()))?
            .clone();
        let current_amount = map
            .get("current_amount")
            .ok_or_else(|| AppError::Deserialization("Missing current_amount".into()))?
            .parse()
            .map_err(|_| AppError::Deserialization("Invalid current_amount".into()))?;
        Ok(Self {
            entry_amount,
            contract_address,
            current_amount,
        })
    }
}

#[derive(Serialize)]
pub struct LobbyExtended {
    pub info: LobbyInfo,
    pub players: Vec<Player>,
    pub pool: Option<LobbyPool>,
}
