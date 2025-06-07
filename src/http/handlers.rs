use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    db::{
        create_room, join_room, leave_room, update_game_state, update_player_state,
        user::create_user,
    },
    models::{GameState, PlayerState, User},
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
    pub user_id: Uuid,
}

pub async fn join_room_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(payload): Json<RoomActionPayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    match join_room(room_id, payload.user_id, state.redis.clone()).await {
        Ok(_) => Ok(Json("Joined room")),
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}

pub async fn leave_room_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(payload): Json<RoomActionPayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    match leave_room(room_id, payload.user_id, state.redis.clone()).await {
        Ok(_) => Ok(Json("Left room")),
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}

#[derive(Deserialize)]
pub struct UpdateGameStatePayload {
    pub new_state: GameState,
}

pub async fn update_game_state_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateGameStatePayload>,
) -> Json<Result<(), String>> {
    let res = update_game_state(room_id, payload.new_state, state.redis.clone()).await;
    Json(res)
}

#[derive(Deserialize)]
pub struct UpdatePlayerStatePayload {
    pub user_id: Uuid,
    pub new_state: PlayerState,
}

pub async fn update_player_state_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(payload): Json<UpdatePlayerStatePayload>,
) -> Json<Result<(), String>> {
    let res = update_player_state(
        room_id,
        payload.user_id,
        payload.new_state,
        state.redis.clone(),
    )
    .await;
    Json(res)
}
