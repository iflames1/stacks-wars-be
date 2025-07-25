use crate::{
    db,
    models::{
        game::{GameState, Player, PlayerState},
        lobby::LobbyServerMessage,
    },
    state::{ChatConnectionInfoMap, ConnectionInfoMap, RedisClient},
    ws::handlers::lobby::message_handler::{broadcast_to_lobby, handler::send_error_to_player},
};
use uuid::Uuid;

pub async fn update_player_state(
    new_state: PlayerState,
    room_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    if let Err(e) =
        db::room::update_player_state(room_id, player.id, new_state.clone(), redis.clone()).await
    {
        tracing::error!("Failed to update state: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        tracing::info!(
            "Player {} updated state to {:?} in room {}",
            player.wallet_address,
            new_state,
            room_id
        );
        let msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            room_id,
            &msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;

        if new_state == PlayerState::NotReady {
            if let Ok(room_info) = db::room::get_room_info(room_id, redis.clone()).await {
                if room_info.state == GameState::InProgress {
                    // revert game state to Waiting
                    let _ = db::room::update_game_state(room_id, GameState::Waiting, redis.clone())
                        .await;
                    let msg = LobbyServerMessage::GameState {
                        state: GameState::Waiting,
                        ready_players: None,
                    };
                    broadcast_to_lobby(room_id, &msg, &connections, None, redis.clone()).await;
                }
            }
        }
    }
}
