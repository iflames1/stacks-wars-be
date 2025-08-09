use uuid::Uuid;

use crate::models::game::LobbyState;

pub struct RedisKey;

impl RedisKey {
    pub fn user(user_id: KeyPart) -> String {
        format!("users:{user_id}")
    }

    pub fn wallet(wallet_address: KeyPart) -> String {
        format!("user_wallets:{wallet_address}")
    }

    pub fn username(username: KeyPart) -> String {
        let username = username.to_string().to_lowercase();
        format!("usernames:{username}")
    }

    pub fn game(game_id: KeyPart) -> String {
        format!("games:{game_id}:data")
    }

    pub fn game_lobbies(game_id: KeyPart) -> String {
        format!("games:{game_id}:lobbies")
    }

    pub fn lobby(lobby_id: KeyPart) -> String {
        format!("lobbies:{lobby_id}:info")
    }

    pub fn lobby_player(lobby_id: KeyPart, player_id: KeyPart) -> String {
        format!("lobbies:{lobby_id}:player:{player_id}")
    }

    pub fn lobby_connected_player(lobby_id: KeyPart, player_id: KeyPart) -> String {
        format!("lobbies:{lobby_id}:connected_player:{player_id}")
    }

    pub fn lobbies_state(state: &LobbyState) -> String {
        format!("lobbies:{}:state", format!("{state:?}").to_lowercase())
    }

    pub fn lobby_chat(lobby_id: KeyPart) -> String {
        format!("lobbies:{lobby_id}:chats")
    }

    // temp keys
    pub fn temp_union() -> String {
        let id = Uuid::new_v4();
        format!("temp:union:{id}")
    }

    pub fn temp_inter() -> String {
        let id = Uuid::new_v4();
        format!("temp:inter:{id}")
    }

    pub fn player_missed_msgs(lobby_id: KeyPart, player_id: KeyPart) -> String {
        format!("lobbies:{lobby_id}:missed_msgs:{player_id}")
    }

    pub fn player_missed_chat_msgs(lobby_id: KeyPart, player_id: KeyPart) -> String {
        format!("lobbies:{lobby_id}:missed_chat_msgs:{player_id}")
    }
}

#[derive(Debug, Clone)]
pub enum KeyPart {
    Id(Uuid),
    Str(String),
    Wildcard,
}

impl From<Uuid> for KeyPart {
    fn from(id: Uuid) -> Self {
        KeyPart::Id(id)
    }
}

impl From<&str> for KeyPart {
    fn from(s: &str) -> Self {
        if s == "*" {
            KeyPart::Wildcard
        } else {
            KeyPart::Str(s.to_string())
        }
    }
}

impl std::fmt::Display for KeyPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyPart::Id(id) => write!(f, "{}", id),
            KeyPart::Str(s) => write!(f, "{}", s),
            KeyPart::Wildcard => write!(f, "*"),
        }
    }
}
