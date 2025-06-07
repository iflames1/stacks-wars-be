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
