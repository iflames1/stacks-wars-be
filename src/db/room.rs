use redis::AsyncCommands;
use uuid::Uuid;

use crate::{
    db::{tx::validate_payment_tx, user::get_user_by_id},
    errors::AppError,
    models::game::{
        ClaimState, GameRoomInfo, GameState, Player, PlayerState, RoomExtended, RoomPool,
        RoomPoolInput,
    },
    state::RedisClient,
};

pub async fn create_room(
    name: String,
    description: Option<String>,
    creator_id: Uuid,
    game_id: Uuid,
    game_name: String,
    pool: Option<RoomPoolInput>,
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
        description,
        creator_id,
        state: GameState::Waiting,
        game_id,
        game_name,
        participants: 1,
        contract_address: pool.as_ref().map(|p| p.contract_address.clone()),
    };

    let creator_user = get_user_by_id(creator_id, redis.clone()).await?;

    if let Some(pool_input) = &pool {
        validate_payment_tx(
            &pool_input.tx_id,
            &creator_user.wallet_address,
            &pool_input.contract_address,
            pool_input.entry_amount,
        )
        .await?;

        let pool_struct = RoomPool {
            entry_amount: pool_input.entry_amount,
            contract_address: pool_input.contract_address.clone(),
            current_amount: pool_input.entry_amount,
        };

        let pool_key = format!("room:{}:pool", room_id);
        let pool_json = serde_json::to_string(&pool_struct)
            .map_err(|e| AppError::Serialization(e.to_string()))?;

        let _: () = conn
            .set(pool_key, pool_json)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    // Serialize room info
    let room_info_json =
        serde_json::to_string(&room_info).map_err(|e| AppError::Serialization(e.to_string()))?;

    let room_player = Player {
        id: creator_user.id,
        wallet_address: creator_user.wallet_address,
        display_name: creator_user.display_name,
        state: PlayerState::Ready,
        used_words: Vec::new(),
        rank: None,
        tx_id: pool.as_ref().map(|p| p.tx_id.to_owned()),
        claim: None,
        prize: None,
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
        .cmd("SADD")
        .arg(format!("game:{}:rooms", game_id))
        .arg(room_id.to_string())
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(room_id)
}

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

pub async fn join_room(
    room_id: Uuid,
    user_id: Uuid,
    tx_id: Option<String>,
    redis: RedisClient,
) -> Result<(), AppError> {
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

    let mut room: GameRoomInfo =
        serde_json::from_str(&room_json).map_err(|e| AppError::Serialization(e.to_string()))?;

    let players_key = format!("room:{}:players", room_id);
    let current_players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    if current_players
        .iter()
        .any(|p| p.contains(&user_id.to_string()))
    {
        return Err(AppError::BadRequest("User already in room".into()));
    }

    let user = get_user_by_id(user_id, redis.clone()).await?;

    // If the room has a pool, validate the payment
    if let Some(contract_address) = &room.contract_address {
        let tx_id = tx_id.clone().ok_or_else(|| {
            AppError::BadRequest("Missing transaction ID for pool-based room".into())
        })?;

        // Fetch the pool info
        let pool_key = format!("room:{}:pool", room_id);
        let pool_json: String = redis::cmd("GET")
            .arg(&pool_key)
            .query_async(&mut *conn)
            .await
            .map_err(|_| AppError::NotFound("Room pool not found".into()))?;

        let pool: RoomPool =
            serde_json::from_str(&pool_json).map_err(|e| AppError::Serialization(e.to_string()))?;

        validate_payment_tx(
            &tx_id,
            &user.wallet_address,
            contract_address,
            pool.entry_amount,
        )
        .await?;

        let updated_pool = RoomPool {
            entry_amount: pool.entry_amount,
            contract_address: contract_address.clone(),
            current_amount: pool.current_amount + pool.entry_amount,
        };

        let new_pool_json = serde_json::to_string(&updated_pool)
            .map_err(|e| AppError::Serialization(e.to_string()))?;

        let _: () = redis::cmd("SET")
            .arg(&pool_key)
            .arg(new_pool_json)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    let room_player = Player {
        id: user.id,
        wallet_address: user.wallet_address,
        display_name: user.display_name,
        state: PlayerState::Ready,
        used_words: Vec::new(),
        rank: None,
        tx_id,
        claim: None,
        prize: None,
    };

    let player_json = serde_json::to_string(&room_player)
        .map_err(|_| AppError::Serialization("Failed to serialize player".into()))?;

    let _: () = redis::cmd("SADD")
        .arg(&players_key)
        .arg(player_json)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    room.participants += 1;

    let updated_json =
        serde_json::to_string(&room).map_err(|e| AppError::Serialization(e.to_string()))?;

    let _: () = redis::cmd("SET")
        .arg(room_key)
        .arg(updated_json)
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

    let (player_to_remove_json, player_obj): (String, Player) = match players.iter().find_map(|p| {
        serde_json::from_str::<Player>(p)
            .ok()
            .filter(|rp| rp.id == user_id)
            .map(|player| (p.clone(), player))
    }) {
        Some(data) => data,
        None => return Err(AppError::BadRequest("User not in room".into())),
    };

    let _: () = redis::cmd("SREM")
        .arg(&players_key)
        .arg(&player_to_remove_json)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    let room_key = format!("room:{}:info", room_id);
    let room_json: String = redis::cmd("GET")
        .arg(&room_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| AppError::NotFound("Room not found".into()))?;

    let mut room: GameRoomInfo =
        serde_json::from_str(&room_json).map_err(|e| AppError::Serialization(e.to_string()))?;

    if room.participants > 0 {
        room.participants -= 1;
    }

    if let Some(contract_address) = &room.contract_address {
        tracing::info!("Deducting entry fee {}", contract_address);
        let pool_key = format!("room:{}:pool", room_id);
        let pool_json: String = redis::cmd("GET")
            .arg(&pool_key)
            .query_async(&mut *conn)
            .await
            .map_err(|_| AppError::NotFound("Room pool not found".into()))?;

        let mut pool: RoomPool =
            serde_json::from_str(&pool_json).map_err(|e| AppError::Serialization(e.to_string()))?;

        // Only deduct if tx_id exists (player actually paid)
        if player_obj.tx_id.is_some() {
            pool.current_amount = pool.current_amount - pool.entry_amount;
        }

        let updated_pool_json =
            serde_json::to_string(&pool).map_err(|e| AppError::Serialization(e.to_string()))?;

        let _: () = redis::cmd("SET")
            .arg(&pool_key)
            .arg(updated_pool_json)
            .query_async(&mut *conn)
            .await
            .map_err(|e| AppError::RedisCommandError(e.into()))?;
    }

    let updated_json =
        serde_json::to_string(&room).map_err(|e| AppError::Serialization(e.to_string()))?;

    let _: () = redis::cmd("SET")
        .arg(room_key)
        .arg(updated_json)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

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

pub async fn update_claim_state(
    room_id: Uuid,
    player_id: Uuid,
    claim_state: ClaimState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let key = format!("room:{}:players", room_id);
    let pool_key = format!("room:{}:pool", room_id);

    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let players_json: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|error| AppError::RedisCommandError(error.into()))?;

    let mut players: Vec<Player> = players_json
        .into_iter()
        .filter_map(|p| serde_json::from_str(&p).ok())
        .collect();

    let Some(player) = players.iter_mut().find(|p| p.id == player_id) else {
        return Err(AppError::BadRequest("Player not found".into()));
    };

    if let ClaimState::Claimed { .. } = &claim_state {
        if let Some(ClaimState::Claimed { .. }) = &player.claim {
            return Err(AppError::BadRequest("Reward already claimed".into()));
        }

        if let Some(prize) = player.prize {
            // Fetch pool
            let pool_json: String = redis::cmd("GET")
                .arg(&pool_key)
                .query_async(&mut *conn)
                .await
                .map_err(|e| AppError::RedisCommandError(e.into()))?;

            let mut pool: RoomPool = serde_json::from_str(&pool_json)
                .map_err(|_| AppError::Deserialization("Failed to parse RoomPool".into()))?;

            if pool.current_amount < prize {
                return Err(AppError::BadRequest("Not enough funds in pool".into()));
            }

            // Deduct prize
            pool.current_amount -= prize;

            // Save updated pool
            let updated_pool = serde_json::to_string(&pool)
                .map_err(|_| AppError::Serialization("Failed to serialize RoomPool".into()))?;

            let _: () = redis::cmd("SET")
                .arg(&pool_key)
                .arg(updated_pool)
                .query_async(&mut *conn)
                .await
                .map_err(|e| AppError::RedisCommandError(e.into()))?;
        } else {
            return Err(AppError::BadRequest("No prize to claim".into()));
        }
    }

    player.claim = Some(claim_state);

    // Replace player set
    let _: () = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    for p in players {
        let player_json = serde_json::to_string(&p)
            .map_err(|_| AppError::Serialization("Failed to serialize player".into()))?;

        let _: () = redis::cmd("SADD")
            .arg(&key)
            .arg(player_json)
            .query_async(&mut *conn)
            .await
            .map_err(|e| AppError::RedisCommandError(e.into()))?;
    }

    Ok(())
}

pub async fn _update_prize(
    room_id: Uuid,
    player_id: Uuid,
    prize: f64,
    redis: RedisClient,
) -> Result<(), AppError> {
    let key = format!("room:{}:players", room_id);
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Get all players
    let players_json: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut players: Vec<Player> = players_json
        .into_iter()
        .filter_map(|p| serde_json::from_str(&p).ok())
        .collect();

    let Some(player) = players.iter_mut().find(|p| p.id == player_id) else {
        return Err(AppError::BadRequest("Player not found".into()));
    };

    player.prize = Some(prize);
    player.claim = Some(ClaimState::NotClaimed);

    // Delete existing set and re-insert all players
    let _: () = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    for p in players {
        let player_json = serde_json::to_string(&p)
            .map_err(|_| AppError::Serialization("Failed to serialize player".into()))?;

        let _: () = redis::cmd("SADD")
            .arg(&key)
            .arg(player_json)
            .query_async(&mut *conn)
            .await
            .map_err(|e| AppError::RedisCommandError(e.into()))?;
    }

    Ok(())
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

pub async fn update_room_player_after_game(
    room_id: Uuid,
    player_id: Uuid,
    rank: usize,
    prize: Option<f64>,
    used_words: Vec<String>,
    redis: RedisClient,
) -> Result<(), AppError> {
    let players_key = format!("room:{}:players", room_id);
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Fetch all players
    let players_json: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut players: Vec<Player> = players_json
        .into_iter()
        .filter_map(|p| serde_json::from_str(&p).ok())
        .collect();

    let Some(player) = players.iter_mut().find(|p| p.id == player_id) else {
        return Err(AppError::BadRequest("Player not found".into()));
    };
    if let Some(prize) = prize {
        player.prize = Some(prize);
        player.claim = Some(ClaimState::NotClaimed);
    } else {
        player.prize = None;
        player.claim = None;
    }
    player.rank = Some(rank);
    player.used_words = used_words;

    // Replace entire player set
    let _: () = redis::cmd("DEL")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    for p in players {
        let player_json = serde_json::to_string(&p)
            .map_err(|_| AppError::Serialization("Failed to serialize player".into()))?;

        let _: () = redis::cmd("SADD")
            .arg(&players_key)
            .arg(player_json)
            .query_async(&mut *conn)
            .await
            .map_err(|e| AppError::RedisCommandError(e.into()))?;
    }

    Ok(())
}
