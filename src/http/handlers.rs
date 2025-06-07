use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    db::{create_room, join_room, leave_room, user::create_user},
    models::User,
    state::AppState,
};

#[derive(Deserialize)]
pub struct CreateUserPayload {
    pub wallet_address: String,
    pub display_name: Option<String>,
}

pub async fn create_user_handler(
    State(state): State<AppState>,
    Json(payload): Json<CreateUserPayload>,
) -> Result<Json<User>, (StatusCode, String)> {
    match create_user(
        payload.wallet_address,
        payload.display_name,
        state.redis.clone(),
    )
    .await
    {
        Ok(user) => Ok(Json(user)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

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

#[derive(Deserialize)]
pub struct RoomActionPayload {
    pub room_id: Uuid,
    pub user_id: Uuid,
}

pub async fn join_room_handler(
    State(state): State<AppState>,
    Json(payload): Json<RoomActionPayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    match join_room(payload.room_id, payload.user_id, state.redis.clone()).await {
        Ok(_) => Ok(Json("Joined room")),
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}

pub async fn leave_room_handler(
    State(state): State<AppState>,
    Json(payload): Json<RoomActionPayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    match leave_room(payload.room_id, payload.user_id, state.redis.clone()).await {
        Ok(_) => Ok(Json("Left room")),
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}
