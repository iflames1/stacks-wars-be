use uuid::Uuid;

use crate::{
    models::{GameRoomInfo, GameState},
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

    let _: () = redis::pipe()
        .cmd("SET")
        .arg(&room_info_key)
        .arg(room_info_json)
        .ignore()
        .cmd("SADD") // Add creator as first player in the room
        .arg(format!("room:{}:players", room_id))
        .arg(creator_id.to_string())
        .query_async(&mut *conn)
        .await
        .unwrap();

    room_id
}
