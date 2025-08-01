use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::AuthClaims,
    db::{
        create_room, join_room, leave_room,
        room::{
            get::get_all_rooms_extended, get_all_rooms, get_room, get_room_extended, get_room_info,
            get_room_players, get_rooms_by_game_id, update_claim_state,
        },
        update_game_state, update_player_state,
    },
    errors::AppError,
    models::{
        game::{
            ClaimState, GameRoomInfo, GameState, Player, PlayerState, RoomExtended, RoomPoolInput,
        },
        lobby::RoomQuery,
    },
    state::AppState,
};

#[derive(Deserialize)]
pub struct CreateRoomPayload {
    pub name: String,
    pub description: Option<String>,
    pub entry_amount: Option<f64>,
    pub contract_address: Option<String>,
    pub tx_id: Option<String>,
    pub game_id: Uuid,
    pub game_name: String,
}

pub async fn create_room_handler(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<CreateRoomPayload>,
) -> Result<Json<Uuid>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    let pool = match (
        payload.entry_amount,
        payload.contract_address.clone(),
        payload.tx_id.clone(),
    ) {
        (Some(entry_amount), Some(contract_address), Some(tx_id)) => Some(RoomPoolInput {
            entry_amount,
            contract_address,
            tx_id,
        }),
        _ => None,
    };

    let room_id = create_room(
        payload.name,
        payload.description,
        user_id,
        payload.game_id,
        payload.game_name,
        pool,
        state.redis.clone(),
        state.telegram_bot.clone(),
    )
    .await
    .map_err(|err| {
        tracing::error!("Error creating room: {}", err);
        err.to_response()
    })?;

    tracing::info!("Room created with ID: {}", room_id);
    Ok(Json(room_id))
}

pub async fn get_room_extended_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<RoomExtended>, (StatusCode, String)> {
    let extended = get_room_extended(room_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving extended room info: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved extended room info for room ID: {}", room_id);
    Ok(Json(extended))
}

fn parse_states(state_param: Option<String>) -> Option<Vec<GameState>> {
    state_param
        .map(|s| {
            s.split(',')
                .filter_map(|state_str| {
                    let trimmed = state_str.trim();
                    match trimmed.to_lowercase().as_str() {
                        "waiting" => Some(GameState::Waiting),
                        "inprogress" | "in_progress" => Some(GameState::InProgress),
                        "finished" => Some(GameState::Finished),
                        _ => {
                            tracing::warn!("Invalid state filter: {}", trimmed);
                            None
                        }
                    }
                })
                .collect()
        })
        .filter(|states: &Vec<GameState>| !states.is_empty())
}

pub async fn get_rooms_by_game_id_handler(
    Path(game_id): Path<Uuid>,
    Query(query): Query<RoomQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<GameRoomInfo>>, (StatusCode, String)> {
    let filter_states = parse_states(query.state);

    let (page, limit) = if let Some(p) = query.page {
        let page = p.max(1);
        let limit = query.limit.unwrap_or(12).min(100); // Default 12, max 100 per page
        (page, limit)
    } else {
        // No pagination - return all items
        (1, u32::MAX)
    };

    let rooms = get_rooms_by_game_id(game_id, filter_states, page, limit, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving rooms by game ID: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved {} rooms for game ID: {}", rooms.len(), game_id);
    Ok(Json(rooms))
}

pub async fn get_room_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<GameRoomInfo>, (StatusCode, String)> {
    let room_info = get_room(room_id, state.redis.clone()).await.map_err(|e| {
        tracing::error!("Error retrieving room info: {}", e);
        e.to_response()
    })?;

    tracing::info!("Retrieved room info for room ID: {}", room_id);
    Ok(Json(room_info))
}

pub async fn get_all_rooms_extended_handler(
    Query(query): Query<RoomQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<RoomExtended>>, (StatusCode, String)> {
    let filter_states = parse_states(query.state);

    let (page, limit) = if let Some(p) = query.page {
        let page = p.max(1); // Ensure page is at least 1
        let limit = query.limit.unwrap_or(12).min(100); // Default 12, max 100 per page
        (page, limit)
    } else {
        // No pagination - return all items
        (1, u32::MAX)
    };

    let rooms = get_all_rooms_extended(filter_states, page, limit, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving all rooms extended: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved {} extended rooms", rooms.len());
    Ok(Json(rooms))
}

pub async fn get_all_rooms_handler(
    Query(query): Query<RoomQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<GameRoomInfo>>, (StatusCode, String)> {
    let filter_states = parse_states(query.state);

    let (page, limit) = if let Some(p) = query.page {
        let page = p.max(1); // Ensure page is at least 1
        let limit = query.limit.unwrap_or(12).min(100); // Default 12, max 100 per page
        (page, limit)
    } else {
        // No pagination - return all items
        (1, u32::MAX)
    };

    let rooms = get_all_rooms(filter_states, page, limit, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving all rooms: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved {} rooms", rooms.len());
    Ok(Json(rooms))
}

pub async fn get_players_handler(
    Path(room_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Player>>, (StatusCode, String)> {
    let players = get_room_players(room_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving players in {room_id}: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved {} players", players.len());
    Ok(Json(players))
}

#[derive(Deserialize)]
pub struct JoinRoomPayload {
    pub tx_id: Option<String>,
}

pub async fn join_room_handler(
    Path(room_id): Path<Uuid>,
    AuthClaims(claims): AuthClaims,
    State(state): State<AppState>,
    Json(payload): Json<JoinRoomPayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    join_room(room_id, user_id, payload.tx_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error joining room {room_id}: {}", e);
            e.to_response()
        })?;

    tracing::info!("Success joining room {room_id}");
    Ok(Json("success"))
}

pub async fn leave_room_handler(
    Path(room_id): Path<Uuid>,
    AuthClaims(claims): AuthClaims,
    State(state): State<AppState>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    leave_room(room_id, user_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error leaving room {room_id}: {}", e);
            e.to_response()
        })?;

    tracing::info!("Success leaving room {room_id}");
    Ok(Json("success"))
}

#[derive(Deserialize)]
pub struct KickPlayerPayload {
    pub player_id: Uuid,
}

pub async fn kick_player_handler(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Path(room_id): Path<Uuid>,
    Json(payload): Json<KickPlayerPayload>,
) -> Result<Json<String>, (StatusCode, String)> {
    let caller_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    let room_info = get_room_info(room_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error getting room info: {}", e);
            e.to_response()
        })?;

    if room_info.creator_id != caller_id {
        return Err({
            tracing::error!("Only the creator can kick players");
            AppError::Unauthorized("Only the creator can kick players".into()).to_response()
        });
    }

    if room_info.state != GameState::Waiting {
        return Err({
            tracing::error!("Cannot kick players when game is in progress or has ended");
            AppError::BadRequest("Cannot kick players when game is in progress or has ended".into())
                .to_response()
        });
    }

    leave_room(room_id, payload.player_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error kicking player: {}", e);
            e.to_response()
        })?;

    tracing::info!("Success kicking player");
    Ok(Json("success".to_string()))
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
    update_game_state(room_id, payload.new_state.clone(), state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error updating game state: {}", e);
            e.to_response()
        })?;

    tracing::info!("Game state update to {:?}", payload.new_state);
    Ok(Json("success"))
}

#[derive(Deserialize)]
pub struct UpdatePlayerStatePayload {
    pub new_state: PlayerState,
}

pub async fn update_player_state_handler(
    Path(room_id): Path<Uuid>,
    AuthClaims(claims): AuthClaims,
    State(state): State<AppState>,
    Json(payload): Json<UpdatePlayerStatePayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    update_player_state(
        room_id,
        user_id,
        payload.new_state.clone(),
        state.redis.clone(),
    )
    .await
    .map_err(|e| {
        tracing::error!("Error updating player state: {}", e);
        e.to_response()
    })?;

    tracing::info!("Player state updated to {:?}", payload.new_state);
    Ok(Json("success"))
}

#[derive(Deserialize)]
pub struct UpdateClaimStatePayload {
    pub claim: ClaimState,
}
pub async fn update_claim_state_handler(
    Path(room_id): Path<Uuid>,
    AuthClaims(claims): AuthClaims,
    State(state): State<AppState>,
    Json(payload): Json<UpdateClaimStatePayload>,
) -> Result<Json<&'static str>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    update_claim_state(room_id, user_id, payload.claim, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error updating game state: {}", e);
            e.to_response()
        })?;

    tracing::info!("Claim state updated for room {room_id}");
    Ok(Json("success"))
}
