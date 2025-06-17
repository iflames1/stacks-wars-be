use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::AuthClaims,
    db::{
        create_room, join_room, leave_room, update_game_state, update_player_state,
        user::create_user,
    },
    errors::AppError,
    models::game::{GameState, PlayerState},
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
) -> Result<Json<String>, (StatusCode, String)> {
    match create_user(
        payload.wallet_address,
        payload.display_name,
        state.redis.clone(),
    )
    .await
    {
        Ok(token) => Ok(Json(token)),
        Err(err) => Err(err.to_response()),
    }
}

#[derive(Deserialize)]
pub struct CreateRoomPayload {
    pub name: String,
    pub max_participants: usize,
}

#[axum::debug_handler]
pub async fn create_room_handler(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<CreateRoomPayload>,
) -> Result<Json<Uuid>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()).to_response())?;

    let room_id = create_room(
        payload.name,
        user_id,
        payload.max_participants,
        state.redis.clone(),
    )
    .await
    .map_err(|err| err.to_response())?;

    Ok(Json(room_id))
}

pub async fn join_room_handler(
    Path(room_id): Path<Uuid>,
    AuthClaims(claims): AuthClaims,
    State(state): State<AppState>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()).to_response())?;

    join_room(room_id, user_id, state.redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    Ok(Json("success"))
}

pub async fn leave_room_handler(
    Path(room_id): Path<Uuid>,
    AuthClaims(claims): AuthClaims,
    State(state): State<AppState>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()).to_response())?;

    leave_room(room_id, user_id, state.redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    Ok(Json("success"))
}

#[derive(Deserialize)]
pub struct UpdateGameStatePayload {
    pub new_state: GameState,
}

pub async fn update_game_state_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(payload): Json<UpdateGameStatePayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    update_game_state(room_id, payload.new_state, state.redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    Ok(Json("success"))
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
) -> Result<Json<&'static str>, (StatusCode, String)> {
    update_player_state(
        room_id,
        payload.user_id,
        payload.new_state,
        state.redis.clone(),
    )
    .await
    .map_err(|e| e.to_response())?;

    Ok(Json("success"))
}
