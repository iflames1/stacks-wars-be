use std::collections::{HashMap, HashSet};

use bb8::PooledConnection;
use bb8_redis::RedisConnectionManager;
use uuid::Uuid;

use crate::{
    db::{game::get::get_game, user::get::get_user_by_id},
    errors::AppError,
    models::{
        game::{LobbyExtended, LobbyInfo, LobbyState, Player, PlayerState},
        redis::{KeyPart, RedisKey},
    },
    state::RedisClient,
};

pub async fn get_lobbies_by_game_id(
    game_id: Uuid,
    lobby_filters: Option<Vec<LobbyState>>,
    page: u32,
    limit: u32,
    redis: RedisClient,
) -> Result<Vec<LobbyInfo>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;
    let offset = ((page.saturating_sub(1)) as usize).saturating_mul(limit as usize);
    let end = offset + (limit as usize) - 1;

    // 1) build the list of lobby IDs (filtered by state if provided)
    let lobby_ids: Vec<String> = if let Some(states) = lobby_filters {
        // Union all the per‐state sorted sets
        let state_keys: Vec<String> = states
            .iter()
            .map(|state| RedisKey::lobbies_state(state))
            .collect();
        let union_key = RedisKey::temp_union();
        let _: () = redis::cmd("ZUNIONSTORE")
            .arg(&union_key)
            .arg(state_keys.len())
            .arg(&state_keys)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;
        let _: Option<()> = redis::cmd("EXPIRE")
            .arg(&union_key)
            .arg(30)
            .query_async(&mut *conn)
            .await
            .ok();

        // Now intersect with the game‐specific set
        let game_key = RedisKey::game_lobbies(KeyPart::Id(game_id));
        let inter_key = RedisKey::temp_inter();
        let _: () = redis::cmd("ZINTERSTORE")
            .arg(&inter_key)
            .arg(2)
            .arg(&game_key)
            .arg(&union_key)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;
        let _: Option<()> = redis::cmd("EXPIRE")
            .arg(&inter_key)
            .arg(30)
            .query_async(&mut *conn)
            .await
            .ok();

        // Page through the intersection
        let ids: Vec<String> = redis::cmd("ZREVRANGE")
            .arg(&inter_key)
            .arg(offset)
            .arg(end)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;

        // Cleanup
        let _: Option<()> = redis::cmd("DEL")
            .arg(&union_key)
            .query_async(&mut *conn)
            .await
            .ok();
        let _: Option<()> = redis::cmd("DEL")
            .arg(&inter_key)
            .query_async(&mut *conn)
            .await
            .ok();
        ids
    } else {
        // No state filter → page straight out of game:{game_id}:lobbies
        redis::cmd("ZREVRANGE")
            .arg(RedisKey::game_lobbies(KeyPart::Id(game_id)))
            .arg(offset)
            .arg(end)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?
    };

    // Filter and collect only valid UUIDs
    let valid_ids: Vec<Uuid> = lobby_ids
        .iter()
        .filter_map(|id_str| Uuid::parse_str(id_str).ok())
        .collect();

    // Batch all HGETALLs using a Redis pipeline
    let mut pipe = redis::pipe();
    for lobby_id in &valid_ids {
        let key = RedisKey::lobby(KeyPart::Id(*lobby_id));
        pipe.cmd("HGETALL").arg(key);
    }

    // Execute pipeline and collect responses
    let results: Vec<HashMap<String, String>> = pipe
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Parse partial lobbies and collect unique creator and game IDs
    let mut partial_lobbies = Vec::new();
    let mut creator_ids = HashSet::new();
    let mut game_ids = HashSet::new();

    for map in results {
        if let Ok((lobby, creator_id, game_id)) = LobbyInfo::from_redis_hash_partial(&map) {
            creator_ids.insert(creator_id);
            game_ids.insert(game_id);
            partial_lobbies.push((lobby, creator_id, game_id));
        }
    }

    // Batch fetch all creators and games
    let mut creators = HashMap::new();
    let mut games = HashMap::new();

    // Fetch creators
    for creator_id in creator_ids {
        if let Ok(creator) = get_user_by_id(creator_id, redis.clone()).await {
            creators.insert(creator_id, creator);
        }
    }

    // Fetch games
    for game_id in game_ids {
        if let Ok(game) = get_game(game_id, redis.clone()).await {
            games.insert(game_id, game);
        }
    }

    // Hydrate creators and games
    let mut out = Vec::new();
    for (mut lobby, creator_id, game_id) in partial_lobbies {
        if let (Some(creator), Some(game)) = (creators.get(&creator_id), games.get(&game_id)) {
            lobby.creator = creator.clone();
            lobby.game = game.clone();
            out.push(lobby);
        }
    }

    Ok(out)
}

pub async fn get_lobby_info(lobby_id: Uuid, redis: RedisClient) -> Result<LobbyInfo, AppError> {
    let redis_clone = redis.clone();
    let mut conn = redis_clone.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = RedisKey::lobby(KeyPart::Id(lobby_id));
    let map: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    if map.is_empty() {
        return Err(AppError::NotFound(format!("Lobby {} not found", lobby_id)));
    }

    // Get partial lobby info and creator ID
    let (mut info, creator_id, game_id) = LobbyInfo::from_redis_hash_partial(&map)?;

    // Hydrate both creator and game concurrently
    let (creator_result, game_result) = tokio::try_join!(
        get_user_by_id(creator_id, redis.clone()),
        get_game(game_id, redis.clone())
    )?;

    info.creator = creator_result;
    info.game = game_result;

    tracing::debug!("{info:?}");

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

    let pattern = RedisKey::lobby_connected_player(KeyPart::Id(lobby_id), KeyPart::Wildcard);
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

pub async fn get_all_lobbies_info(
    lobby_filters: Option<Vec<LobbyState>>,
    page: u32,
    limit: u32,
    redis: RedisClient,
) -> Result<Vec<LobbyInfo>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis conn timed out".into()),
    })?;

    let offset = ((page.saturating_sub(1)) as usize).saturating_mul(limit as usize);
    let end = offset + (limit as usize) - 1;
    let uuids: Vec<Uuid> = fetch_lobby_uuids(&mut conn, lobby_filters, offset, end).await?;

    // Batch all HGETALLs using a Redis pipeline
    let mut pipe = redis::pipe();
    for uuid in &uuids {
        let key = RedisKey::lobby(KeyPart::Id(*uuid));
        pipe.cmd("HGETALL").arg(key);
    }

    // Execute pipeline and collect responses
    let results: Vec<HashMap<String, String>> = pipe
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Parse partial lobbies and collect unique creator and game IDs
    let mut partial_lobbies = Vec::new();
    let mut creator_ids = HashSet::new();
    let mut game_ids = HashSet::new();

    for map in results {
        if let Ok((lobby, creator_id, game_id)) = LobbyInfo::from_redis_hash_partial(&map) {
            creator_ids.insert(creator_id);
            game_ids.insert(game_id);
            partial_lobbies.push((lobby, creator_id, game_id));
        }
    }

    // Batch fetch all creators and games
    let mut creators = HashMap::new();
    let mut games = HashMap::new();

    // Fetch creators
    for creator_id in creator_ids {
        if let Ok(creator) = get_user_by_id(creator_id, redis.clone()).await {
            creators.insert(creator_id, creator);
        }
    }

    // Fetch games
    for game_id in game_ids {
        if let Ok(game) = get_game(game_id, redis.clone()).await {
            games.insert(game_id, game);
        }
    }

    // Hydrate creators and games
    let mut out = Vec::new();
    for (mut lobby, creator_id, game_id) in partial_lobbies {
        if let (Some(creator), Some(game)) = (creators.get(&creator_id), games.get(&game_id)) {
            lobby.creator = creator.clone();
            lobby.game = game.clone();
            out.push(lobby);
        }
    }

    Ok(out)
}

pub async fn get_lobby_players(
    lobby_id: Uuid,
    players_filter: Option<PlayerState>,
    redis: RedisClient,
) -> Result<Vec<Player>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // find all player hashes for this lobby
    let pattern = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Wildcard);
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

        // apply optional state filter
        if let Some(ref state_filter) = players_filter {
            if &player.state != state_filter {
                continue;
            }
        }

        players.push(player);
    }

    Ok(players)
}

pub async fn get_lobby_extended(
    lobby_id: Uuid,
    players_filter: Option<PlayerState>,
    redis: RedisClient,
) -> Result<LobbyExtended, AppError> {
    let lobby = get_lobby_info(lobby_id, redis.clone()).await?;
    let players = get_lobby_players(lobby_id, players_filter, redis.clone()).await?;

    Ok(LobbyExtended { lobby, players })
}

pub async fn get_all_lobbies_extended(
    lobby_filters: Option<Vec<LobbyState>>,
    players_filter: Option<PlayerState>,
    page: u32,
    limit: u32,
    redis: RedisClient,
) -> Result<Vec<LobbyExtended>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let offset = ((page.saturating_sub(1)) as usize).saturating_mul(limit as usize);
    let end = offset + (limit as usize) - 1;

    let uuids: Vec<Uuid> = fetch_lobby_uuids(&mut conn, lobby_filters, offset, end).await?;

    let mut out = Vec::with_capacity(uuids.len());
    for lobby_id in uuids {
        let lobby = get_lobby_info(lobby_id, redis.clone()).await?;

        let players = get_lobby_players(lobby_id, players_filter.clone(), redis.clone()).await?;

        out.push(LobbyExtended { lobby, players });
    }

    Ok(out)
}

async fn fetch_lobby_uuids(
    conn: &mut PooledConnection<'_, RedisConnectionManager>,
    lobby_filters: Option<Vec<LobbyState>>,
    offset: usize,
    end: usize,
) -> Result<Vec<Uuid>, AppError> {
    let ids: Vec<String> = if let Some(states) = lobby_filters {
        let keys: Vec<String> = states
            .iter()
            .map(|state| RedisKey::lobbies_state(state))
            .collect();
        let union = RedisKey::temp_union();
        let _: () = redis::cmd("ZUNIONSTORE")
            .arg(&union)
            .arg(keys.len())
            .arg(&keys)
            .query_async(&mut **conn)
            .await
            .map_err(AppError::RedisCommandError)?;
        let _: Option<()> = redis::cmd("EXPIRE")
            .arg(&union)
            .arg(30)
            .query_async(&mut **conn)
            .await
            .ok();

        let out: Vec<String> = redis::cmd("ZREVRANGE")
            .arg(&union)
            .arg(offset)
            .arg(end)
            .query_async(&mut **conn)
            .await
            .map_err(AppError::RedisCommandError)?;

        // cleanup
        let _: Option<()> = redis::cmd("DEL")
            .arg(&union)
            .query_async(&mut **conn)
            .await
            .ok();
        out
    } else {
        redis::cmd("ZREVRANGE")
            .arg("lobbies:all")
            .arg(offset)
            .arg(end)
            .query_async(&mut **conn)
            .await
            .map_err(AppError::RedisCommandError)?
    };

    let mut uuids: Vec<Uuid> = ids
        .into_iter()
        .filter_map(|s| Uuid::parse_str(&s).ok())
        .collect();
    uuids.dedup();
    Ok(uuids)
}
