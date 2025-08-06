use chrono::Utc;
use redis::AsyncCommands;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    db::{
        game::patch::update_game_active_lobby, tx::validate_payment_tx, user::get::get_user_by_id,
    },
    errors::AppError,
    models::{
        game::{ClaimState, LobbyInfo, LobbyPool, LobbyState, Player, PlayerState},
        redis::{KeyPart, RedisKey},
    },
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

    let lobby_key = RedisKey::lobby(KeyPart::Id(lobby_id));
    let lobby_map: HashMap<String, String> = conn
        .hgetall(&lobby_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if lobby_map.is_empty() {
        return Err(AppError::NotFound(format!("Lobby {} not found", lobby_id)));
    }
    let (lobby, _creator_id, _game_id) = LobbyInfo::from_redis_hash_partial(&lobby_map)?;

    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(user_id));
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

        let pool_key = RedisKey::lobby_pool(KeyPart::Id(lobby_id));
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

    let lobby_key = RedisKey::lobby(KeyPart::Id(lobby_id));
    let m: HashMap<String, String> = conn
        .hgetall(&lobby_key)
        .await
        .map_err(AppError::RedisCommandError)?;
    if m.is_empty() {
        return Err(AppError::NotFound(format!("Lobby {} not found", lobby_id)));
    }
    let (info, creator_id, _game_id) = LobbyInfo::from_redis_hash_partial(&m)?;

    if creator_id == user_id {
        // delete lobby hash
        let pattern = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Wildcard);
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;
        if keys.len() == 1 {
            let pool_key = RedisKey::lobby_pool(KeyPart::Id(lobby_id));
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

    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(user_id));
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
            let pool_key = RedisKey::lobby_pool(KeyPart::Id(lobby_id));
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

    let lobby_key = RedisKey::lobby(KeyPart::Id(lobby_id));

    // Read the existing state and game_id
    let old_state_str: String = conn
        .hget(&lobby_key, "state")
        .await
        .map_err(AppError::RedisCommandError)?;
    let game_id_str: String = conn
        .hget(&lobby_key, "game_id")
        .await
        .map_err(AppError::RedisCommandError)?;

    let old_state = old_state_str
        .parse::<LobbyState>()
        .map_err(|_| AppError::Deserialization("Invalid old state".into()))?;
    let game_id = game_id_str
        .parse::<Uuid>()
        .map_err(|_| AppError::Deserialization("Invalid game_id".into()))?;

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
    let old_z = RedisKey::lobbies_state(&old_state);
    let new_z = RedisKey::lobbies_state(&new_state);

    let _: () = conn
        .zrem(&old_z, lobby_id.to_string())
        .await
        .map_err(AppError::RedisCommandError)?;
    let _: () = conn
        .zadd(&new_z, lobby_id.to_string(), score)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Update active lobby count when transitioning to/from Finished
    if new_state == LobbyState::Finished
        && (old_state == LobbyState::Waiting || old_state == LobbyState::InProgress)
    {
        update_game_active_lobby(game_id, false, redis.clone()).await?;
    } else if (new_state == LobbyState::InProgress || new_state == LobbyState::Waiting)
        && old_state == LobbyState::Finished
    {
        update_game_active_lobby(game_id, true, redis.clone()).await?;
    }

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
    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(user_id));

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

    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(user_id));
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

pub async fn _update_player_prize(
    lobby_id: Uuid,
    player_id: Uuid,
    prize: f64,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(player_id));

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

    let player = Player::from_redis_hash(&map)?;

    if player.prize == Some(prize) {
        return Ok(());
    }

    let claim_json = serde_json::to_string(&ClaimState::NotClaimed)
        .map_err(|e| AppError::Serialization(e.to_string()))?;

    let updates = vec![("prize", prize.to_string()), ("claim", claim_json)];

    let hset_args: Vec<(&str, &str)> = updates.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let _: () = conn
        .hset_multiple(&player_key, &hset_args)
        .await
        .map_err(AppError::RedisCommandError)?;

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

    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(player_id));
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
