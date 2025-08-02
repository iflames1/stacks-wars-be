use std::collections::HashMap;

use uuid::Uuid;

use crate::{
    errors::AppError,
    models::{
        game::{GameRoomInfo, GameState, LobbyInfo, Player, PlayerState, RoomExtended, RoomPool},
        lobby::PaginationMeta,
    },
    state::RedisClient,
};

pub async fn get_rooms_by_game_id(
    game_id: Uuid,
    filter_states: Option<Vec<GameState>>,
    page: u32,
    limit: u32,
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

            if let Some(ref state_filters) = filter_states {
                if !state_filters.contains(&info.state) {
                    continue;
                }
            }

            rooms.push(info);
        }
    }

    rooms.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Apply pagination
    let total_count = rooms.len() as u32;
    let total_pages = if total_count == 0 {
        1
    } else if limit == u32::MAX {
        1
    } else {
        (total_count + limit - 1) / limit
    };
    let offset = (page - 1) * limit;

    let paginated_rooms = rooms
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    let _pagination_meta = PaginationMeta {
        page,
        limit,
        total_count,
        total_pages,
        has_next: page < total_pages,
        has_previous: page > 1,
    };

    Ok(paginated_rooms)
}

pub async fn get_lobby_info(lobby_id: Uuid, redis: RedisClient) -> Result<LobbyInfo, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;
    let key = format!("lobby:{}", lobby_id);

    // fetch base hash
    let map: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;
    if map.is_empty() {
        return Err(AppError::NotFound(format!("Lobby {} not found", lobby_id)));
    }

    let info = LobbyInfo::from_redis_hash(&map)?;

    Ok(info)
}

pub async fn get_connected_players(
    lobby_id: Uuid,
    redis: RedisClient,
) -> Result<Vec<Player>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let pattern = format!("lobby:{}:connected_player:*", lobby_id);
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(&pattern)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let mut players = Vec::with_capacity(keys.len());
    for key in keys {
        let pmap: HashMap<String, String> = redis::cmd("HGETALL")
            .arg(&key)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;

        let player = Player::from_redis_hash(&pmap)?;
        players.push(player);
    }

    Ok(players)
}

pub async fn get_all_rooms(
    filter_states: Option<Vec<GameState>>,
    page: u32,
    limit: u32,
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

        if let Some(ref state_filters) = filter_states {
            if !state_filters.contains(&room.state) {
                continue;
            }
        }

        rooms.push(room);
    }

    // Sort by created_at in descending order (latest first)
    rooms.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Apply pagination
    let total_count = rooms.len() as u32;
    let total_pages = if total_count == 0 {
        1
    } else if limit == u32::MAX {
        1
    } else {
        (total_count + limit - 1) / limit
    };
    let offset = (page - 1) * limit;

    let paginated_rooms = rooms
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    let _pagination_meta = PaginationMeta {
        page,
        limit,
        total_count,
        total_pages,
        has_next: page < total_pages,
        has_previous: page > 1,
    };

    Ok(paginated_rooms)
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

pub async fn get_all_rooms_extended(
    filter_states: Option<Vec<GameState>>,
    page: u32,
    limit: u32,
    redis: RedisClient,
) -> Result<Vec<RoomExtended>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("room:*:info")
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut rooms_extended = Vec::new();

    for key in keys {
        // Extract room_id from key (format: "room:{room_id}:info")
        let room_id_str = key
            .strip_prefix("room:")
            .and_then(|s| s.strip_suffix(":info"));

        if let Some(room_id_str) = room_id_str {
            if let Ok(room_id) = Uuid::parse_str(room_id_str) {
                // Get room info first to check state filter
                let info_value: String = redis::cmd("GET")
                    .arg(&key)
                    .query_async(&mut *conn)
                    .await
                    .map_err(|e| AppError::RedisCommandError(e.into()))?;

                let info: GameRoomInfo = serde_json::from_str(&info_value)
                    .map_err(|_| AppError::Deserialization("Invalid room info".to_string()))?;

                // Apply state filter
                if let Some(ref state_filters) = filter_states {
                    if !state_filters.contains(&info.state) {
                        continue;
                    }
                }

                // Get players for this room
                let players_key = format!("room:{}:players", room_id);
                let raw_players: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(&players_key)
                    .query_async(&mut *conn)
                    .await
                    .map_err(|e| AppError::RedisCommandError(e.into()))?;

                let mut players = Vec::new();
                for p in raw_players {
                    let player: Player = serde_json::from_str(&p)
                        .map_err(|_| AppError::Deserialization("Invalid player JSON".into()))?;
                    players.push(player);
                }

                // Get pool if exists
                let pool = if let Some(addr) = &info.contract_address {
                    if !addr.is_empty() {
                        let pool_key = format!("room:{}:pool", room_id);
                        let pool_result: Result<String, redis::RedisError> = redis::cmd("GET")
                            .arg(&pool_key)
                            .query_async(&mut *conn)
                            .await;

                        match pool_result {
                            Ok(pool_value) => {
                                serde_json::from_str(&pool_value).map(Some).map_err(|_| {
                                    AppError::Deserialization("Invalid room pool JSON".into())
                                })?
                            }
                            Err(_) => None, // Pool doesn't exist, which is fine
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let room_extended = RoomExtended {
                    info,
                    players,
                    pool,
                };

                rooms_extended.push(room_extended);
            }
        }
    }

    // Sort by created_at in descending order (latest first)
    rooms_extended.sort_by(|a, b| b.info.created_at.cmp(&a.info.created_at));

    // Apply pagination
    let total_count = rooms_extended.len() as u32;
    let total_pages = if total_count == 0 {
        1
    } else if limit == u32::MAX {
        1
    } else {
        (total_count + limit - 1) / limit
    };
    let offset = (page - 1) * limit;

    let paginated_rooms = rooms_extended
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    let _pagination_meta = PaginationMeta {
        page,
        limit,
        total_count,
        total_pages,
        has_next: page < total_pages,
        has_previous: page > 1,
    };

    Ok(paginated_rooms)
}
