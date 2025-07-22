use crate::models::{
    game::{GameState, Player, PlayerState},
    user::User,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JoinState {
    Idle,
    Pending,
    Allowed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub user: User,
    pub state: JoinState,
}

#[derive(Deserialize)]
pub struct RoomQuery {
    pub state: Option<String>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct PaginatedResponse<T> {
    pub data: Vec<T>,
    pub pagination: PaginationMeta,
}

#[derive(Serialize)]
pub struct PaginationMeta {
    pub page: u32,
    pub limit: u32,
    pub total_count: u32,
    pub total_pages: u32,
    pub has_next: bool,
    pub has_previous: bool,
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
    Ping {
        ts: u64,
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
    Pong {
        ts: u64,
        pong: u64,
    },
}

// Add this to your lobby.rs file (assuming it exists)
impl LobbyServerMessage {
    /// Determines if this message should be queued for offline players
    pub fn should_queue(&self) -> bool {
        match self {
            // Time-sensitive messages that should NOT be queued
            LobbyServerMessage::Countdown { .. } => false,
            LobbyServerMessage::Pong { .. } => false,

            // Important messages that SHOULD be queued
            LobbyServerMessage::Error { .. } => true,
            LobbyServerMessage::Allowed { .. } => true,
            LobbyServerMessage::GameState { .. } => true,
            LobbyServerMessage::PlayersNotReady { .. } => true,
            LobbyServerMessage::PlayerJoined { .. } => true,
            LobbyServerMessage::PlayerLeft { .. } => true,
            LobbyServerMessage::PlayerKicked { .. } => true,
            LobbyServerMessage::Rejected { .. } => true,
            LobbyServerMessage::PendingPlayers { .. } => true,
            LobbyServerMessage::NotifyKicked => true,
            LobbyServerMessage::PlayerUpdated { .. } => true,
            LobbyServerMessage::Pending { .. } => true,
        }
    }
}
