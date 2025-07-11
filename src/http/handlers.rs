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
        create_room,
        game::{add_game, get_all_games, get_game},
        join_room, leave_room,
        room::{
            get_all_rooms, get_room, get_room_extended, get_room_info, get_room_players,
            get_rooms_by_game_id, update_claim_state,
        },
        update_game_state, update_player_state,
        user::{create_user, get_user_by_id},
    },
    errors::AppError,
    models::{
        User,
        game::{
            ClaimState, GameRoomInfo, GameState, GameType, Player, PlayerState, RoomExtended,
            RoomPoolInput,
        },
    },
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
        payload.wallet_address.clone(),
        payload.display_name,
        state.redis.clone(),
    )
    .await
    {
        Ok(token) => {
            tracing::info!(
                "User created with wallet address: {}",
                payload.wallet_address
            );
            Ok(Json(token))
        }
        Err(err) => {
            tracing::error!("Error creating user: {}", err);
            Err(err.to_response())
        }
    }
}

pub async fn get_user_handler(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<User>, (StatusCode, String)> {
    let user = get_user_by_id(user_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving user: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved user: {:?}", user);

    Ok(Json(user))
}

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
        state.bot.clone(),
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

#[derive(Deserialize)]
pub struct RoomQuery {
    state: Option<GameState>,
}
pub async fn get_rooms_by_game_id_handler(
    Path(game_id): Path<Uuid>,
    Query(query): Query<RoomQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<GameRoomInfo>>, (StatusCode, String)> {
    let rooms = get_rooms_by_game_id(game_id, query.state, state.redis.clone())
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

pub async fn get_all_rooms_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<GameRoomInfo>>, (StatusCode, String)> {
    let rooms = get_all_rooms(state.redis.clone()).await.map_err(|e| {
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

#[derive(Debug, Deserialize)]
pub struct AddGamePayload {
    pub name: String,
    pub description: String,
    pub image_url: String,
    pub tags: Option<Vec<String>>,
    pub min_players: u8,
}
pub async fn add_game_handler(
    State(state): State<AppState>,
    Json(payload): Json<AddGamePayload>,
) -> Result<Json<Uuid>, (StatusCode, String)> {
    let id = Uuid::new_v4();
    let game = GameType {
        id,
        name: payload.name,
        description: payload.description,
        image_url: payload.image_url,
        tags: payload.tags,
        min_players: payload.min_players,
    };

    let id = add_game(game, state.redis.clone()).await.map_err(|e| {
        tracing::error!("Error adding new game: {}", e);
        e.to_response()
    })?;

    tracing::info!("Success adding game {id}");
    Ok(Json(id))
}

pub async fn get_game_handler(
    Path(game_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<GameType>, (StatusCode, String)> {
    let game = get_game(game_id, state.redis.clone()).await.map_err(|e| {
        tracing::error!("Error retrieving {} game: {}", game_id, e);
        e.to_response()
    })?;

    tracing::info!("Success retrieving {game_id} game");
    Ok(Json(game))
}

pub async fn get_all_games_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<GameType>>, (StatusCode, String)> {
    let games = get_all_games(state.redis.clone()).await.map_err(|e| {
        tracing::error!("Error retrieving all games: {}", e);
        e.to_response()
    })?;

    tracing::info!("Success retrieving all game");
    Ok(Json(games))
}
