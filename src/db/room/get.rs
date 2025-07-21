use uuid::Uuid;

use crate::{
    errors::AppError,
    models::game::{GameRoomInfo, GameState, Player, PlayerState, RoomExtended, RoomPool},
    state::RedisClient,
};

pub async fn get_rooms_by_game_id(
    game_id: Uuid,
    filter_state: Option<GameState>,
    redis: RedisClient,
) -> Result<Vec<GameRoomInfo>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game:{}:rooms", game_id);
    let room_ids: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut rooms = Vec::new();

    for id_str in room_ids {
        if let Ok(uuid) = Uuid::parse_str(&id_str) {
            let room_key = format!("room:{}:info", uuid);
            let json: String = redis::cmd("GET")
                .arg(&room_key)
                .query_async(&mut *conn)
                .await
                .map_err(|e| AppError::RedisCommandError(e.into()))?;

            let info: GameRoomInfo = serde_json::from_str(&json)
                .map_err(|_| AppError::Deserialization("Invalid room info".into()))?;

            if let Some(ref state_filter) = filter_state {
                if &info.state != state_filter {
                    continue;
                }
            }

            rooms.push(info);
        }
    }

    Ok(rooms)
}

pub async fn get_room(room_id: Uuid, redis: RedisClient) -> Result<GameRoomInfo, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("room:{}:info", room_id);
    let json: String = redis::cmd("GET")
        .arg(key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;
    let info: GameRoomInfo = serde_json::from_str(&json)
        .map_err(|_| AppError::Deserialization("Invalid room info JSON".into()))?;

    Ok(info)
}

pub async fn get_all_rooms(
    filter_state: Option<GameState>,
    redis: RedisClient,
) -> Result<Vec<GameRoomInfo>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("room:*:info")
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut rooms = Vec::new();
    for key in keys {
        let value: String = redis::cmd("GET")
            .arg(key)
            .query_async(&mut *conn)
            .await
            .map_err(|e| AppError::RedisCommandError(e.into()))?;
        let room: GameRoomInfo = serde_json::from_str(&value)
            .map_err(|_| AppError::Deserialization("Invalid room info".to_string()))?;

        if let Some(ref state_filter) = filter_state {
            if &room.state != state_filter {
                continue;
            }
        }

        rooms.push(room);
    }

    Ok(rooms)
}

pub async fn get_room_players(room_id: Uuid, redis: RedisClient) -> Result<Vec<Player>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("room:{}:players", room_id);
    let raw_players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut players = Vec::new();
    for p in raw_players {
        let player: Player = serde_json::from_str(&p)
            .map_err(|_| AppError::Deserialization("Invalid player JSON".into()))?;
        players.push(player);
    }

    Ok(players)
}

pub async fn get_ready_room_players(
    room_id: Uuid,
    redis: RedisClient,
) -> Result<Vec<Player>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("room:{}:players", room_id);
    let raw_players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut players = Vec::new();
    for p in raw_players {
        let player: Player = serde_json::from_str(&p)
            .map_err(|_| AppError::Deserialization("Invalid player JSON".into()))?;

        if player.state == PlayerState::Ready {
            players.push(player);
        }
    }

    Ok(players)
}

pub async fn get_room_info(room_id: Uuid, redis: RedisClient) -> Result<GameRoomInfo, AppError> {
    let key = format!("room:{}:info", room_id);
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;
    let value: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| AppError::NotFound("Room not found".into()))?;

    serde_json::from_str(&value)
        .map_err(|_| AppError::Deserialization("Invalid room info JSON".into()))
}

pub async fn get_room_pool(room_id: Uuid, redis: RedisClient) -> Result<RoomPool, AppError> {
    let key = format!("room:{}:pool", room_id);
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let value: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| AppError::NotFound("Room pool not found".into()))?;

    serde_json::from_str(&value)
        .map_err(|_| AppError::Deserialization("Invalid room pool JSON".into()))
}

pub async fn get_room_extended(
    room_id: Uuid,
    redis: RedisClient,
) -> Result<RoomExtended, AppError> {
    let info = get_room_info(room_id, redis.clone()).await?;
    let players = get_room_players(room_id, redis.clone()).await?;

    let pool = if let Some(addr) = &info.contract_address {
        if !addr.is_empty() {
            Some(get_room_pool(room_id, redis).await?)
        } else {
            None
        }
    } else {
        None
    };

    Ok(RoomExtended {
        info,
        players,
        pool,
    })
}
