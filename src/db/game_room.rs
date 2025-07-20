use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::{
    errors::AppError,
    games::lexi_wars::{rules::RuleContext, utils::generate_random_letter},
    models::{
        game::{GameData, GameRoom, GameRoomInfo, Player, RoomPool},
        word_loader::WORD_LIST,
    },
    state::RedisClient,
};

const GAME_ROOM_TTL: u64 = 21600; // 6 hours in seconds

pub async fn store_game_room(
    room_id: Uuid,
    room: &GameRoom,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game_room:{}", room_id);
    let serialized =
        serde_json::to_string(room).map_err(|e| AppError::Serialization(e.to_string()))?;

    // Store with 6 hour TTL
    let _: () = redis::cmd("SETEX")
        .arg(&key)
        .arg(GAME_ROOM_TTL)
        .arg(&serialized)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::debug!("Stored game room {} in Redis with 6h TTL", room_id);
    Ok(())
}

pub async fn get_game_room(room_id: Uuid, redis: RedisClient) -> Result<GameRoom, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game_room:{}", room_id);
    let serialized: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| AppError::NotFound("Game room not found".into()))?;

    let room: GameRoom =
        serde_json::from_str(&serialized).map_err(|e| AppError::Deserialization(e.to_string()))?;

    Ok(room)
}

pub async fn update_game_room<F>(
    room_id: Uuid,
    redis: RedisClient,
    update_fn: F,
) -> Result<GameRoom, AppError>
where
    F: FnOnce(&mut GameRoom),
{
    let mut room = get_game_room(room_id, redis.clone()).await?;
    update_fn(&mut room);
    store_game_room(room_id, &room, redis).await?;
    Ok(room)
}

pub async fn create_or_get_game_room(
    room_info: &GameRoomInfo,
    players: Vec<Player>,
    pool: Option<RoomPool>,
    redis: RedisClient,
) -> Result<GameRoom, AppError> {
    // Try to get existing room first
    if let Ok(existing_room) = get_game_room(room_info.id, redis.clone()).await {
        return Ok(existing_room);
    }

    // Create new room if it doesn't exist
    let word_list = WORD_LIST.clone();
    let room = GameRoom {
        info: room_info.clone(),
        players,
        data: GameData::LexiWar { word_list },
        eliminated_players: vec![],
        current_turn_id: room_info.creator_id,
        used_words: HashMap::new(),
        used_words_global: HashSet::new(),
        rule_context: RuleContext {
            min_word_length: 4,
            random_letter: generate_random_letter(),
        },
        rule_index: 0,
        pool,
    };

    store_game_room(room_info.id, &room, redis).await?;
    tracing::info!("Created new game room {} in Redis", room_info.id);
    Ok(room)
}

pub async fn _remove_game_room(room_id: Uuid, redis: RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game_room:{}", room_id);
    let _: () = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::debug!("Removed game room {} from Redis", room_id);
    Ok(())
}

pub async fn _extend_game_room_ttl(room_id: Uuid, redis: RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game_room:{}", room_id);
    let _: () = redis::cmd("EXPIRE")
        .arg(&key)
        .arg(GAME_ROOM_TTL)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::debug!("Extended TTL for game room {}", room_id);
    Ok(())
}
