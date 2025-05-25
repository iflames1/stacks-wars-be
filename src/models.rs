use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Player {
    pub id: Uuid,
    pub username: Option<String>,
}

pub struct GameRoom {
    pub id: Uuid,
    pub players: Vec<Player>,
}

pub type Rooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;
