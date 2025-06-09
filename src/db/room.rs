use uuid::Uuid;

use crate::models::PlayerState;
use crate::{
    db::user::get_user_by_id,
    models::{GameRoomInfo, GameState, RoomPlayer},
    state::RedisClient,
};

pub async fn create_room(
    name: String,
    creator_id: Uuid,
    max_participants: usize,
    redis: RedisClient,
) -> Uuid {
    let room_id = Uuid::new_v4();
    let mut conn = redis.get().await.unwrap();

    let room_info = GameRoomInfo {
        id: room_id,
        name,
        creator_id,
        max_participants,
        state: GameState::Waiting,
    };

    // Store serialized room info
    let room_info_key = format!("room:{}:info", room_id);
    let room_info_json = serde_json::to_string(&room_info).unwrap();

    let creator_user = get_user_by_id(creator_id, redis.clone())
        .await
        .expect("Failed to get creator user");

    let room_player = RoomPlayer {
        id: creator_user.id,
        wallet_address: creator_user.wallet_address,
        display_name: creator_user.display_name,
        state: PlayerState::Ready,
        used_words: Vec::new(),
        rank: None,
    };

    let room_player_json = serde_json::to_string(&room_player).unwrap();

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
        .unwrap();

    room_id
}

pub async fn join_room(room_id: Uuid, user_id: Uuid, redis: RedisClient) -> Result<(), String> {
    let mut conn = redis
        .get()
        .await
        .map_err(|_| "Failed to get Redis connection")?;

    let room_key = format!("room:{}:info", room_id);
    let room_json: String = redis::cmd("GET")
        .arg(&room_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to get room")?;

    let room: GameRoomInfo = serde_json::from_str(&room_json).map_err(|_| "Invalid room data")?;

    let players_key = format!("room:{}:players", room_id);
    let current_players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to fetch players")?;

    if current_players.len() >= room.max_participants {
        return Err("Room is full".into());
    }

    if current_players
        .iter()
        .any(|p| p.contains(&user_id.to_string()))
    {
        return Err("User already in room".into());
    }

    let user = get_user_by_id(user_id, redis.clone())
        .await
        .ok_or("User not found")?;

    let room_player = RoomPlayer {
        id: user.id,
        wallet_address: user.wallet_address,
        display_name: user.display_name,
        state: PlayerState::NotReady,
        used_words: Vec::new(),
        rank: None,
    };

    let player_json = serde_json::to_string(&room_player).unwrap();

    let _: () = redis::cmd("SADD")
        .arg(&players_key)
        .arg(player_json)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to add player to room")?;

    Ok(())
}

pub async fn leave_room(room_id: Uuid, user_id: Uuid, redis: RedisClient) -> Result<(), String> {
    let mut conn = redis
        .get()
        .await
        .map_err(|_| "Failed to get Redis connection")?;

    let players_key = format!("room:{}:players", room_id);

    let players: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to fetch players")?;

    let Some(player_to_remove) = players.iter().find(|p| {
        serde_json::from_str::<RoomPlayer>(p)
            .map(|rp| rp.id == user_id)
            .unwrap_or(false)
    }) else {
        return Err("User not in room".into());
    };

    let _: () = redis::cmd("SREM")
        .arg(&players_key)
        .arg(player_to_remove)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to remove player from room")?;

    Ok(())
}

// should only creator update state
pub async fn update_game_state(
    room_id: Uuid,
    new_state: GameState,
    redis: RedisClient,
) -> Result<(), String> {
    let mut conn = redis
        .get()
        .await
        .map_err(|_| "Failed to get Redis connection")?;

    let room_info_key = format!("room:{}:info", room_id);
    let info_json: String = redis::cmd("GET")
        .arg(&room_info_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to get room info")?;

    let mut info: GameRoomInfo =
        serde_json::from_str(&info_json).map_err(|_| "Invalid room data")?;
    if info.state == new_state {
        return Ok(());
    }
    info.state = new_state;

    let updated_json = serde_json::to_string(&info).map_err(|_| "Serialization error")?;

    let _: () = redis::cmd("SET")
        .arg(&room_info_key)
        .arg(updated_json)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to update room state")?;

    Ok(())
}

pub async fn update_player_state(
    room_id: Uuid,
    user_id: Uuid,
    new_state: PlayerState,
    redis: RedisClient,
) -> Result<(), String> {
    let mut conn = redis
        .get()
        .await
        .map_err(|_| "Failed to get Redis connection")?;

    let players_key = format!("room:{}:players", room_id);

    let players_json: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to fetch players")?;

    let mut players: Vec<RoomPlayer> = players_json
        .into_iter()
        .filter_map(|p| serde_json::from_str(&p).ok())
        .collect();

    let Some(player) = players.iter_mut().find(|p| p.id == user_id) else {
        return Err("Player not found in room".into());
    };

    if player.state == new_state {
        return Ok(());
    }
    player.state = new_state;

    let _: () = redis::cmd("DEL")
        .arg(&players_key)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to clear players set")?;

    for p in players {
        let player_json = serde_json::to_string(&p).map_err(|_| "Serialization error")?;
        let _: () = redis::cmd("SADD")
            .arg(&players_key)
            .arg(player_json)
            .query_async(&mut *conn)
            .await
            .map_err(|_| "Failed to add player to room")?;
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

pub async fn get_room_players(room_id: Uuid, redis: &RedisClient) -> Option<Vec<RoomPlayer>> {
    let key = format!("room:{}:players", room_id);
    let mut conn = redis.get().await.ok()?;
    let value: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .ok()?;
    serde_json::from_str(&value).ok()
}
