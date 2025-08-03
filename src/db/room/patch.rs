use chrono::Utc;
use uuid::Uuid;

use crate::{
    db::{tx::validate_payment_tx, user::get_user_by_id},
    errors::AppError,
    models::game::{
        ClaimState, GameRoomInfo, GameState, LobbyState, Player, PlayerState, RoomPool,
    },
    state::RedisClient,
};

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

use redis::AsyncCommands;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    db::tx::validate_payment_tx,
    db::user::get_user_by_id,
    errors::AppError,
    models::game::{LobbyInfo, LobbyPool, Player, PlayerState},
    state::RedisClient,
};

pub async fn join_lobby(
    lobby_id: Uuid,
    user_id: Uuid,
    tx_id: Option<String>,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let lobby_key = format!("lobby:{}", lobby_id);
    let lobby_map: HashMap<String, String> = conn
        .hgetall(&lobby_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if lobby_map.is_empty() {
        return Err(AppError::NotFound(format!("Lobby {} not found", lobby_id)));
    }
    let lobby = LobbyInfo::from_redis_hash(&lobby_map)?;

    let player_key = format!("lobby:{}:player:{}", lobby_id, user_id);
    if conn
        .exists(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?
    {
        return Err(AppError::BadRequest("User already in lobby".into()));
    }

    // If there's a pool, validate tx and update it
    if let Some(addr) = &lobby.contract_address {
        let tx = tx_id
            .clone()
            .ok_or_else(|| AppError::BadRequest("Missing transaction ID for pool".into()))?;

        let pool_key = format!("lobby:{}:pool", lobby_id);
        let pool_map: HashMap<String, String> = conn
            .hgetall(&pool_key)
            .await
            .map_err(AppError::RedisCommandError)?;
        if pool_map.is_empty() {
            return Err(AppError::NotFound("Lobby pool not found".into()));
        }
        let pool = LobbyPool::from_redis_hash(&pool_map)?;

        // Fetch user for wallet address
        let user = get_user_by_id(user_id, redis.clone()).await?;
        validate_payment_tx(&tx, &user.wallet_address, addr, pool.entry_amount).await?;

        // Increment pool
        let _: () = conn
            .hincr(&pool_key, "current_amount", pool.entry_amount as i64)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    let user = get_user_by_id(user_id, redis.clone()).await?;
    let new_player = Player {
        id: user.id,
        wallet_address: user.wallet_address,
        state: PlayerState::Ready,
        display_name: user.display_name,
        username: user.username,
        wars_point: user.wars_point,
        used_words: None,
        rank: None,
        tx_id,
        claim: None,
        prize: None,
    };
    let player_hash = new_player.to_redis_hash();
    let player_fields: Vec<(&str, &str)> = player_hash
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let _: () = conn
        .hset_multiple(&player_key, &player_fields)
        .await
        .map_err(AppError::RedisCommandError)?;

    let _: () = conn
        .hincr(&lobby_key, "participants", 1)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn leave_lobby(
    lobby_id: Uuid,
    user_id: Uuid,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let lobby_key = format!("lobby:{}", lobby_id);
    let m: HashMap<String, String> = conn
        .hgetall(&lobby_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if m.is_empty() {
        return Err(AppError::NotFound(format!("Lobby {} not found", lobby_id)));
    }
    let info = LobbyInfo::from_redis_hash(&m)?;

    if info.creator_id == user_id {
        // delete lobby hash
        let pattern = format!("lobby:{}:player:*", lobby_id);
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;
        if keys.len() == 1 {
            let pool_key = format!("lobby:{}:pool", lobby_id);
            let _: () = conn
                .del(&lobby_key)
                .await
                .map_err(AppError::RedisCommandError)?;
            let _: () = conn
                .del(&pool_key)
                .await
                .map_err(AppError::RedisCommandError)?;

            // delete all player hashes

            for key in keys {
                let _: () = conn.del(key).await.map_err(AppError::RedisCommandError)?;
            }
        } else {
            return Err(AppError::BadRequest(
                "Creator cannot leave lobby with players".into(),
            ));
        }

        return Ok(());
    }

    let player_key = format!("lobby:{}:player:{}", lobby_id, user_id);
    if !conn
        .exists(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?
    {
        return Err(AppError::BadRequest("User not in lobby".into()));
    }
    let pm: HashMap<String, String> = conn
        .hgetall(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    let player = Player::from_redis_hash(&pm)?;
    let _: () = conn
        .del(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    let _: () = conn
        .hincr(&lobby_key, "participants", -1)
        .await
        .map_err(AppError::RedisCommandError)?;

    if let Some(_addr) = &info.contract_address {
        if player.tx_id.is_some() {
            let pool_key = format!("lobby:{}:pool", lobby_id);
            let ph: HashMap<String, String> = conn
                .hgetall(&pool_key)
                .await
                .map_err(AppError::RedisCommandError)?;
            if !ph.is_empty() {
                let pool = LobbyPool::from_redis_hash(&ph)?;
                let _: () = conn
                    .hincr(&pool_key, "current_amount", -(pool.entry_amount as i64))
                    .await
                    .map_err(AppError::RedisCommandError)?;
            }
        }
    }

    Ok(())
}

pub async fn update_lobby_state(
    lobby_id: Uuid,
    new_state: LobbyState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let lobby_key = format!("lobby:{}", lobby_id);

    // Read the existing state
    let old_state_str: String = conn
        .hget(&lobby_key, "state")
        .await
        .map_err(AppError::RedisCommandError)?;
    let old_state = old_state_str
        .parse::<LobbyState>()
        .map_err(|_| AppError::Deserialization("Invalid old state".into()))?;

    // No-op if state is unchanged
    if old_state == new_state {
        return Ok(());
    }

    let _: () = conn
        .hset(&lobby_key, "state", format!("{:?}", new_state))
        .await
        .map_err(AppError::RedisCommandError)?;

    // Move the lobby ID between the old & new state ZSETs
    let score = Utc::now().timestamp();
    let old_z = format!("lobbies:{}", format!("{:?}", old_state).to_lowercase());
    let new_z = format!("lobbies:{}", format!("{:?}", new_state).to_lowercase());

    let _: () = conn
        .zrem(&old_z, lobby_id.to_string())
        .await
        .map_err(AppError::RedisCommandError)?;
    let _: () = conn
        .zadd(&new_z, lobby_id.to_string(), score)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn update_player_state(
    lobby_id: Uuid,
    user_id: Uuid,
    new_state: PlayerState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Build the player hash key
    let player_key = format!("lobby:{}:player:{}", lobby_id, user_id);

    // Fetch the existing hash
    let map: HashMap<String, String> = conn
        .hgetall(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if map.is_empty() {
        return Err(AppError::NotFound(format!(
            "Player {} not found in lobby {}",
            user_id, lobby_id
        )));
    }

    let player = Player::from_redis_hash(&map)?;

    // Short‐circuit if no change
    if player.state == new_state {
        return Ok(());
    }

    // Update only the "state" field in Redis
    let _: () = conn
        .hset(&player_key, "state", format!("{:?}", new_state))
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn update_claim_state(
    lobby_id: Uuid,
    user_id: Uuid,
    new_claim: ClaimState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let player_key = format!("lobby:{}:player:{}", lobby_id, user_id);
    let map: HashMap<String, String> = conn
        .hgetall(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if map.is_empty() {
        return Err(AppError::NotFound(format!(
            "Player {} not found in lobby {}",
            user_id, lobby_id
        )));
    }

    let current = Player::from_redis_hash(&map)?;
    if current.claim.as_ref() == Some(&new_claim) {
        return Ok(());
    }

    let claim_json =
        serde_json::to_string(&new_claim).map_err(|e| AppError::Serialization(e.to_string()))?;
    let _: () = conn
        .hset(&player_key, "claim", claim_json)
        .await
        .map_err(AppError::RedisCommandError)?;

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

pub async fn update_lexi_wars_player(
    lobby_id: Uuid,
    player_id: Uuid,
    rank: usize,
    prize: Option<f64>,
    used_words: Vec<String>,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let player_key = format!("lobby:{}:player:{}", lobby_id, player_id);
    let map: HashMap<String, String> = conn
        .hgetall(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if map.is_empty() {
        return Err(AppError::NotFound(format!(
            "Player {} not found in lobby {}",
            player_id, lobby_id
        )));
    }

    let mut player = Player::from_redis_hash(&map)?;
    player.rank = Some(rank);
    player.used_words = Some(used_words.clone());
    if let Some(amount) = prize {
        player.prize = Some(amount);
        player.claim = Some(ClaimState::NotClaimed);
    }

    let mut updates = Vec::new();
    updates.push(("rank", rank.to_string()));
    updates.push((
        "used_words",
        serde_json::to_string(&used_words).map_err(|e| AppError::Serialization(e.to_string()))?,
    ));
    if let Some(amount) = player.prize {
        updates.push(("prize", amount.to_string()));
    }
    if let Some(ref c) = player.claim {
        updates.push((
            "claim",
            serde_json::to_string(c).map_err(|e| AppError::Serialization(e.to_string()))?,
        ));
    }

    let hset_args: Vec<(&str, &str)> = updates
        .iter()
        .map(|(k, v)| (k.as_ref(), v.as_str()))
        .collect();
    let _: () = conn
        .hset_multiple(&player_key, &hset_args)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn update_connected_players(
    room_id: Uuid,
    connected_players: Vec<Player>,
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

    room.connected_players = connected_players;

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
