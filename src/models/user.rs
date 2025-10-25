use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use uuid::Uuid;

use crate::models::game::Player;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Uuid,
    pub wallet_address: String,
    pub wars_point: f64,

    pub username: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UserWarsPoints {
    pub id: Uuid,
    pub user_id: Uuid,
    pub season_id: i32,
    pub points: f64,
    pub rank_badge: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserV2 {
    pub id: Uuid,
    pub wallet_address: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub trust_rating: f64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub wars_point: UserWarsPoints,
}

impl From<Player> for User {
    fn from(player: Player) -> Self {
        player.into_user()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,    // user ID
    pub wallet: String, // wallet address
    pub exp: usize,     // expiration time
}
