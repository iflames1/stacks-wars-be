use axum::{Json, extract::State};
use serde::Deserialize;
use uuid::Uuid;

use crate::{db::room::create_room, state::AppState};

#[derive(Deserialize)]
pub struct CreateRoomPayload {
    pub name: String,
    pub creator_id: Uuid,
    pub max_participants: usize,
}

pub async fn create_room_handler(
    State(state): State<AppState>,
    Json(payload): Json<CreateRoomPayload>,
) -> Json<Uuid> {
    let room_id = create_room(
        payload.name,
        payload.creator_id,
        payload.max_participants,
        state.redis.clone(),
    )
    .await;

    Json(room_id)
}
