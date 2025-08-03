use uuid::Uuid;

use crate::models::game::LobbyState;

pub struct RedisKey;

impl RedisKey {
    pub fn user(user_id: Uuid) -> String {
        format!("user:{user_id}")
    }

    pub fn wallet(wallet_address: &str) -> String {
        format!("user_wallet:{wallet_address}")
    }

    pub fn username(username: &str) -> String {
        let username = username.to_lowercase();
        format!("username:{username}")
    }

    pub fn lobby(id: Uuid) -> String {
        format!("lobby:{}", id)
    }

    pub fn lobby_pool(id: Uuid) -> String {
        format!("lobby:{}:pool", id)
    }

    pub fn lobby_players(id: Uuid) -> String {
        format!("lobby:{}:players", id)
    }

    pub fn lobbies_state(state: &LobbyState) -> String {
        format!("lobbies:{}", format!("{:?}", state).to_lowercase())
    }
}
