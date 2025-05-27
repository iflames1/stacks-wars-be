use std::collections::HashSet;
use uuid::Uuid;

use crate::ws::rules::RuleContext;

#[derive(Debug, Clone)]
pub struct Player {
    pub id: Uuid,
    pub username: Option<String>,
}

#[derive(Debug)]
pub struct GameRoom {
    pub id: Uuid,
    pub players: Vec<Player>,
    pub current_turn_id: Uuid,
    pub used_words: HashSet<String>,
    pub rule_context: RuleContext,
    pub rule_index: usize,
}
