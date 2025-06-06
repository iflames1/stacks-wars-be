use std::collections::HashSet;
use uuid::Uuid;

use crate::ws::rules::RuleContext;

#[derive(Debug, Clone)]
pub struct Player {
    pub id: Uuid,
    pub wallet_address: String,
    pub display_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct QueryParams {
    pub username: String,
}

#[derive(Debug)]
pub struct GameRoom {
    pub id: Uuid,
    pub players: Vec<Player>,
    pub eliminated_players: Vec<Player>,
    pub current_turn_id: Uuid,
    pub used_words: HashSet<String>,
    pub rule_context: RuleContext,
    pub rule_index: usize,
    pub game_over: bool,
    pub rankings: Vec<(Uuid, usize)>,
}

#[derive(serde::Serialize)]
pub struct Standing {
    pub username: String,
    pub rank: usize,
}
