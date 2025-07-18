use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    games::lexi_wars::rules::RuleContext,
    models::{User, lobby::JoinState},
};

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
    pub display_name: Option<String>,
    pub state: PlayerState,
    pub rank: Option<usize>,
    pub used_words: Vec<String>,
    pub tx_id: Option<String>,
    pub claim: Option<ClaimState>,
    pub prize: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomPool {
    pub entry_amount: f64,
    pub contract_address: String,
    pub current_amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomPoolInput {
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

#[derive(Serialize, Deserialize, Debug, Clone)]
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
}

#[derive(Serialize)]
pub struct RoomExtended {
    pub info: GameRoomInfo,
    pub players: Vec<Player>,
    pub pool: Option<RoomPool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LobbyClientMessage {
    UpdatePlayerState {
        new_state: PlayerState,
    },
    UpdateGameState {
        new_state: GameState,
    },
    LeaveRoom,
    KickPlayer {
        player_id: Uuid,
        wallet_address: String,
        display_name: Option<String>,
    },
    RequestJoin,
    PermitJoin {
        user_id: Uuid,
        allow: bool,
    },
    JoinLobby {
        tx_id: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PendingJoin {
    pub user: User,
    pub state: JoinState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LobbyServerMessage {
    PlayerJoined {
        players: Vec<Player>,
    },
    PlayerLeft {
        players: Vec<Player>,
    },
    PlayerUpdated {
        players: Vec<Player>,
    },
    PlayerKicked {
        player_id: Uuid,
        wallet_address: String,
        display_name: Option<String>,
    },
    NotifyKicked,
    Countdown {
        time: u64,
    },
    GameState {
        state: GameState,
        ready_players: Option<Vec<Uuid>>,
    },
    PendingPlayers {
        pending_players: Vec<PendingJoin>,
    },
    PlayersNotReady {
        players: Vec<Player>,
    },
    Allowed,
    Rejected,
    Pending,
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LexiWarsClientMessage {
    WordEntry { word: String },
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
}
