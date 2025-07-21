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
