use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Player {
    pub id: Uuid,
    pub username: Option<String>,
}

#[derive(Debug)]
pub struct GameRoom {
    pub id: Uuid,
    pub players: Vec<Player>,
}
