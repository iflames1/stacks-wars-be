use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{errors::AppError, games::lexi_wars::rules::RuleContext, models::User};

#[derive(Deserialize)]
pub struct WsQueryParams {
    pub user_id: Uuid,
}

#[derive(Debug)]
pub enum GameData {
    LexiWar { word_list: Arc<HashSet<String>> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameType {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub image_url: String,
    pub min_players: u8,
    pub active_lobbies: u16,
    pub tags: Option<Vec<String>>,
}

impl GameType {
    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("id".into(), self.id.to_string());
        map.insert("name".into(), self.name.clone());
        map.insert("description".into(), self.description.clone());
        map.insert("image_url".into(), self.image_url.clone());
        map.insert("min_players".into(), self.min_players.to_string());
        map.insert("active_lobbies".into(), self.active_lobbies.to_string());
        if let Some(ref tags) = self.tags {
            map.insert("tags".into(), serde_json::to_string(tags).unwrap());
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

            name: map
                .get("name")
                .ok_or_else(|| AppError::Deserialization("Missing name".into()))?
                .clone(),

            description: map
                .get("description")
                .ok_or_else(|| AppError::Deserialization("Missing description".into()))?
                .clone(),

            image_url: map
                .get("image_url")
                .ok_or_else(|| AppError::Deserialization("Missing image_url".into()))?
                .clone(),

            min_players: map
                .get("min_players")
                .ok_or_else(|| AppError::Deserialization("Missing min_players".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid min_players".into()))?,

            active_lobbies: map
                .get("active_lobbies")
                .ok_or_else(|| AppError::Deserialization("Missing active_lobbies".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid active_lobbies".into()))?,

            tags: map
                .get("tags")
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()),
        })
    }
}

#[derive(Debug)]
pub struct LexiWars {
    pub info: LobbyInfo,
    pub players: Vec<Player>,
    pub data: GameData,
    pub used_words_in_lobby: HashSet<String>,
    pub used_words: HashMap<Uuid, Vec<String>>,
    pub rule_context: RuleContext,
    pub rule_index: usize,
    pub current_turn_id: Uuid,
    pub eliminated_players: Vec<Player>,
    pub connected_player_ids: Vec<Uuid>,
    pub connected_players_count: usize,
    pub game_started: bool,
    pub current_rule: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    pub wallet_address: String,
    pub rank: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
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
#[serde(tag = "status", content = "data", rename_all = "camelCase")]
pub enum ClaimState {
    Claimed { tx_id: String },
    NotClaimed,
}

impl ClaimState {
    pub fn matches_filter(&self, filter: &ClaimState) -> bool {
        match (self, filter) {
            (ClaimState::NotClaimed, ClaimState::NotClaimed) => true,
            (ClaimState::Claimed { .. }, ClaimState::Claimed { .. }) => true,
            _ => false,
        }
    }

    pub fn is_claimed(&self) -> bool {
        matches!(self, ClaimState::Claimed { .. })
    }

    pub fn is_not_claimed(&self) -> bool {
        matches!(self, ClaimState::NotClaimed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub id: Uuid,
    pub state: PlayerState,

    // Game-specific fields only
    pub rank: Option<usize>,
    pub used_words: Option<Vec<String>>,
    pub tx_id: Option<String>,
    pub claim: Option<ClaimState>,
    pub prize: Option<f64>,

    // Hydrated user data (not stored in Redis)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
}

impl Player {
    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("id".into(), self.id.to_string());
        map.insert("state".into(), format!("{:?}", self.state));

        if let Some(ref rank) = self.rank {
            map.insert("rank".into(), rank.to_string());
        }
        if let Some(ref used_words) = self.used_words {
            if let Ok(json) = serde_json::to_string(used_words) {
                map.insert("used_words".into(), json);
            }
        }
        if let Some(ref tx_id) = self.tx_id {
            map.insert("tx_id".into(), tx_id.clone());
        }
        if let Some(ref claim) = self.claim {
            if let Ok(json) = serde_json::to_string(claim) {
                map.insert("claim".into(), json);
            }
        }
        if let Some(ref prize) = self.prize {
            map.insert("prize".into(), prize.to_string());
        }

        map
    }

    pub fn from_redis_hash(data: &HashMap<String, String>) -> Result<Self, AppError> {
        let id = data
            .get("id")
            .ok_or_else(|| AppError::Deserialization("Missing player id".into()))?
            .parse::<Uuid>()
            .map_err(|e| AppError::Deserialization(format!("Invalid player id: {}", e)))?;

        let state = data
            .get("state")
            .ok_or_else(|| AppError::Deserialization("Missing player state".into()))?
            .parse::<PlayerState>()
            .map_err(|e| AppError::Deserialization(format!("Invalid player state: {}", e)))?;

        let rank = data.get("rank").and_then(|v| v.parse::<usize>().ok());

        let used_words = data
            .get("used_words")
            .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok());

        let tx_id = data.get("tx_id").cloned();

        let claim = data
            .get("claim")
            .and_then(|v| serde_json::from_str::<ClaimState>(v).ok());

        let prize = data.get("prize").and_then(|v| v.parse::<f64>().ok());

        Ok(Player {
            id,
            state,
            rank,
            used_words,
            tx_id,
            claim,
            prize,
            user: None, // Will be hydrated separately
        })
    }

    // Helper to create a new player with minimal data
    pub fn new(user_id: Uuid, tx_id: Option<String>) -> Self {
        Player {
            id: user_id,
            state: PlayerState::Ready,
            rank: None,
            used_words: None,
            tx_id,
            claim: None,
            prize: None,
            user: None,
        }
    }

    pub fn into_user(self) -> User {
        self.user.unwrap_or_else(|| {
            tracing::warn!(
                "Player {} is missing user data, creating minimal User",
                self.id
            );
            User {
                id: self.id,
                wallet_address: String::new(),
                wars_point: 0.0,
                username: None,
                display_name: None,
            }
        })
    }

    pub fn get_user(&self) -> Option<&User> {
        self.user.as_ref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbyPoolInput {
    pub entry_amount: f64,
    pub contract_address: String,
    pub tx_id: String,
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
            "Finished" => Ok(LobbyState::Finished),
            other => Err(format!("Unknown LobbyState: {}", other)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayerLobbyInfo {
    #[serde(flatten)]
    pub lobby: LobbyInfo,
    pub prize_amount: Option<f64>,
    pub rank: Option<usize>,
    pub claim_state: Option<ClaimState>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LobbyInfo {
    pub id: Uuid,
    pub name: String,

    pub creator: User,
    pub state: LobbyState,
    pub game: GameType,
    pub participants: usize,
    pub created_at: DateTime<Utc>,

    pub description: Option<String>,
    pub contract_address: Option<String>,
    pub entry_amount: Option<f64>,
    pub current_amount: Option<f64>,
}

impl LobbyInfo {
    pub fn to_redis_hash(&self) -> Vec<(String, String)> {
        let mut fields = vec![
            ("id".into(), self.id.to_string()),
            ("name".into(), self.name.clone()),
            ("creator_id".into(), self.creator.id.to_string()),
            ("state".into(), format!("{:?}", self.state)),
            ("game_id".into(), self.game.id.to_string()),
            ("participants".into(), self.participants.to_string()),
            ("created_at".into(), self.created_at.to_rfc3339()),
        ];
        if let Some(desc) = &self.description {
            fields.push(("description".into(), desc.clone()));
        }
        if let Some(addr) = &self.contract_address {
            fields.push(("contract_address".into(), addr.clone()));
        }
        if let Some(entry) = self.entry_amount {
            fields.push(("entry_amount".into(), entry.to_string()));
        }
        if let Some(current) = self.current_amount {
            fields.push(("current_amount".into(), current.to_string()));
        }
        fields
    }

    pub fn from_redis_hash_partial(
        map: &HashMap<String, String>,
    ) -> Result<(Self, Uuid, Uuid), AppError> {
        let creator_id = map
            .get("creator_id")
            .ok_or_else(|| AppError::Deserialization("Missing creator_id".into()))?
            .parse()
            .map_err(|_| AppError::Deserialization("Invalid UUID for creator_id".into()))?;

        let game_id = map
            .get("game_id")
            .ok_or_else(|| AppError::Deserialization("Missing game_id".into()))?
            .parse()
            .map_err(|_| AppError::Deserialization("Invalid UUID for game_id".into()))?;

        // Create placeholder structs - will be replaced during hydration
        let placeholder_creator = User {
            id: creator_id,
            wallet_address: String::new(),
            wars_point: 0.0,
            username: None,
            display_name: None,
        };

        let placeholder_game = GameType {
            id: game_id,
            name: String::new(),
            description: String::new(),
            image_url: String::new(),
            min_players: 0,
            active_lobbies: 0,
            tags: None,
        };

        let lobby = Self {
            id: map
                .get("id")
                .ok_or_else(|| AppError::Deserialization("Missing id".into()))?
                .parse()
                .map_err(|_| AppError::Deserialization("Invalid UUID for id".into()))?,
            name: map
                .get("name")
                .ok_or_else(|| AppError::Deserialization("Missing name".into()))?
                .clone(),
            creator: placeholder_creator,
            state: map
                .get("state")
                .ok_or_else(|| AppError::Deserialization("Missing state".into()))?
                .parse::<LobbyState>()
                .map_err(|_| AppError::Deserialization("Invalid state".into()))?,
            game: placeholder_game,
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
            entry_amount: map.get("entry_amount").and_then(|s| s.parse().ok()),
            current_amount: map.get("current_amount").and_then(|s| s.parse().ok()),
        };

        Ok((lobby, creator_id, game_id))
    }
}

#[derive(Serialize, Debug)]
pub struct LobbyExtended {
    pub lobby: LobbyInfo,
    pub players: Vec<Player>,
}

#[derive(Deserialize)]
pub struct LobbyQuery {
    pub lobby_state: Option<String>,
    pub player_state: Option<String>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

pub fn parse_lobby_states(state_param: Option<String>) -> Option<Vec<LobbyState>> {
    state_param
        .map(|s| {
            s.split(',')
                .filter_map(|state_str| {
                    let trimmed = state_str.trim();
                    match trimmed {
                        "waiting" => Some(LobbyState::Waiting),
                        "inProgress" => Some(LobbyState::InProgress),
                        "finished" => Some(LobbyState::Finished),
                        _ => {
                            tracing::warn!("Invalid state filter: {}", trimmed);
                            None
                        }
                    }
                })
                .collect()
        })
        .filter(|states: &Vec<LobbyState>| !states.is_empty())
}

#[derive(Deserialize)]
pub struct PlayerQuery {
    pub player_state: Option<String>,
}

pub fn parse_player_state(param: Option<String>) -> Option<PlayerState> {
    param.and_then(|s| match s.to_lowercase().as_str() {
        "ready" => Some(PlayerState::Ready),
        "notready" | "not_ready" => Some(PlayerState::NotReady),
        other => {
            tracing::warn!("Invalid player_state filter: {}", other);
            None
        }
    })
}
