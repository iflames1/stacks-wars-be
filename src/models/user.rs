use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::game::Player;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub wallet_address: String,
    pub display_name: Option<String>,
}

impl From<Player> for User {
    fn from(player: Player) -> Self {
        User {
            id: player.id,
            wallet_address: player.wallet_address,
            display_name: player.display_name,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,    // user ID
    pub wallet: String, // wallet address
    pub exp: usize,     // expiration time
}
