use uuid::Uuid;

use crate::{
    db::user::get_user_by_id,
    errors::AppError,
    models::game::{GameRoomInfo, GameState, Player, PlayerState},
    state::RedisClient,
};

pub async fn create_room(
    name: String,
    creator_id: Uuid,
    max_participants: usize,
    redis: RedisClient,
) -> Result<Uuid, AppError> {
    let room_id = Uuid::new_v4();
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let room_info = GameRoomInfo {
        id: room_id,
        name,
        creator_id,
        max_participants,
        state: GameState::Waiting,
    };

    // Serialize room info
    let room_info_json =
        serde_json::to_string(&room_info).map_err(|e| AppError::Serialization(e.to_string()))?;

    let creator_user = get_user_by_id(creator_id, redis.clone()).await?;

    let room_player = Player {
        id: creator_user.id,
        wallet_address: creator_user.wallet_address,
        display_name: creator_user.display_name,
        state: PlayerState::Ready,
        used_words: Vec::new(),
        rank: None,
    };

    let room_info_key = format!("room:{}:info", room_id);

    let room_player_json =
        serde_json::to_string(&room_player).map_err(|e| AppError::Serialization(e.to_string()))?;

    let _: () = redis::pipe()
        .cmd("SET")
        .arg(&room_info_key)
        .arg(room_info_json)
        .ignore()
        .cmd("SADD")
        .arg(format!("room:{}:players", room_id))
        .arg(room_player_json)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(room_id)
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

pub async fn get_all_rooms(redis: RedisClient) -> Result<Vec<GameRoomInfo>, AppError> {
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
        rooms.push(room);
    }

    Ok(rooms)
}

pub async fn get_players(room_id: Uuid, redis: RedisClient) -> Result<Vec<Player>, AppError> {
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

pub async fn join_room(room_id: Uuid, user_id: Uuid, redis: RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let room_key = format!("room:{}:info", room_id);
    let room_json: String = redis::cmd("GET")
        .arg(&room_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| AppError::NotFound("Room not found".into()))?;

    let room: GameRoomInfo =
        serde_json::from_str(&room_json).map_err(|e| AppError::Serialization(e.to_string()))?;

    let players_key = format!("room:{}:players", room_id);
    let current_players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    if current_players.len() >= room.max_participants {
        return Err(AppError::BadRequest("Room is full".into()));
    }

    if current_players
        .iter()
        .any(|p| p.contains(&user_id.to_string()))
    {
        return Err(AppError::BadRequest("User already in room".into()));
    }

    let user = get_user_by_id(user_id, redis.clone()).await?;

    let room_player = Player {
        id: user.id,
        wallet_address: user.wallet_address,
        display_name: user.display_name,
        state: PlayerState::NotReady,
        used_words: Vec::new(),
        rank: None,
    };

    let player_json = serde_json::to_string(&room_player)
        .map_err(|_| AppError::Serialization("Failed to serialize player".into()))?;

    let _: () = redis::cmd("SADD")
        .arg(&players_key)
        .arg(player_json)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    Ok(())
}

pub async fn leave_room(room_id: Uuid, user_id: Uuid, redis: RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let players_key = format!("room:{}:players", room_id);

    let players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    let Some(player_to_remove) = players.iter().find_map(|p| {
        serde_json::from_str::<Player>(p)
            .ok()
            .filter(|rp| rp.id == user_id)
            .map(|_| p) // we return the original JSON string
    }) else {
        return Err(AppError::BadRequest("User not in room".into()));
    };

    let _: () = redis::cmd("SREM")
        .arg(&players_key)
        .arg(player_to_remove)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    Ok(())
}

// should only creator update state
pub async fn update_game_state(
    room_id: Uuid,
    new_state: GameState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let room_info_key = format!("room:{}:info", room_id);
    let info_json: String = redis::cmd("GET")
        .arg(&room_info_key)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    let mut info: GameRoomInfo = serde_json::from_str(&info_json)
        .map_err(|_| AppError::Serialization("Invalid room info".to_string()))?;
    if info.state == new_state {
        return Ok(());
    }
    info.state = new_state;

    let updated_json = serde_json::to_string(&info)
        .map_err(|_| AppError::Deserialization("Serialization error".to_string()))?;

    let _: () = redis::cmd("SET")
        .arg(&room_info_key)
        .arg(updated_json)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    Ok(())
}

pub async fn update_player_state(
    room_id: Uuid,
    user_id: Uuid,
    new_state: PlayerState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let players_key = format!("room:{}:players", room_id);

    let players_json: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    let mut players: Vec<Player> = players_json
        .into_iter()
        .filter_map(|p| serde_json::from_str(&p).ok())
        .collect();

    let Some(player) = players.iter_mut().find(|p| p.id == user_id) else {
        return Err(AppError::Unauthorized("Player not found in room".into()));
    };

    if player.state == new_state {
        return Ok(());
    }
    player.state = new_state;

    let _: () = redis::cmd("DEL")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    for p in players {
        let player_json = serde_json::to_string(&p)
            .map_err(|_| AppError::Deserialization("serialization error".to_string()))?;
        let _: () = redis::cmd("SADD")
            .arg(&players_key)
            .arg(player_json)
            .query_async(&mut *conn)
            .await
            .map_err(|error| AppError::RedisCommandError(error.into()))?;
    }

    Ok(())
}

pub async fn get_room_info(room_id: Uuid, redis: &RedisClient) -> Option<GameRoomInfo> {
    let key = format!("room:{}:info", room_id);
    let mut conn = redis.get().await.ok()?;
    let value: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .ok()?;
    serde_json::from_str(&value).ok()
}

pub async fn get_room_players(room_id: Uuid, redis: &RedisClient) -> Option<Vec<Player>> {
    let key = format!("room:{}:players", room_id);
    let mut conn = redis.get().await.ok()?;

    let values: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .ok()?;

    let players: Vec<Player> = values
        .into_iter()
        .filter_map(|v| serde_json::from_str(&v).ok())
        .collect();

    Some(players)
}

pub async fn update_room_player_after_game(
    room_id: Uuid,
    player_id: Uuid,
    new_rank: usize,
    new_used_words: Vec<String>,
    redis: RedisClient,
) -> Result<(), String> {
    let players_key = format!("room:{}:players", room_id);
    let mut conn = redis
        .get()
        .await
        .map_err(|_| "Failed to get Redis connection")?;

    // Fetch all players
    let players_json: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to fetch players from Redis")?;

    let mut players: Vec<Player> = players_json
        .into_iter()
        .filter_map(|p| serde_json::from_str(&p).ok())
        .collect();

    let Some(player) = players.iter_mut().find(|p| p.id == player_id) else {
        return Err("Player not found in room".into());
    };

    player.rank = Some(new_rank);
    player.used_words = new_used_words;

    // Replace entire player set
    let _: () = redis::cmd("DEL")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to clear old players")?;

    for p in players {
        let p_json = serde_json::to_string(&p).map_err(|_| "Serialization error")?;
        let _: () = redis::cmd("SADD")
            .arg(&players_key)
            .arg(p_json)
            .query_async(&mut *conn)
            .await
            .map_err(|_| "Failed to re-add player")?;
    }

    Ok(())
}
