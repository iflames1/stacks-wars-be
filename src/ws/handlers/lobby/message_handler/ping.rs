use crate::{
    models::{game::Player, lobby::LobbyServerMessage},
    state::{ConnectionInfoMap, RedisClient},
    ws::handlers::lobby::message_handler::handler::send_to_player,
};
use chrono::Utc;

pub async fn ping(ts: u64, player: &Player, connections: &ConnectionInfoMap, redis: &RedisClient) {
    let now = Utc::now().timestamp_millis() as u64;
    let pong = now.saturating_sub(ts);

    let msg = LobbyServerMessage::Pong { ts, pong };
    send_to_player(player.id, &connections, &msg, &redis).await
}
