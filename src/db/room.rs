use uuid::Uuid;

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
