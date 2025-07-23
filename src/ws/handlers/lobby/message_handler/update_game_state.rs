use crate::{
    db,
    models::{
        game::{GameState, Player, PlayerState},
        lobby::LobbyServerMessage,
    },
    state::{ConnectionInfoMap, RedisClient},
    ws::handlers::{
        lobby::message_handler::{
            broadcast_to_lobby,
            handler::{send_error_to_player, send_to_player},
        },
        utils::remove_connection,
    },
};
use axum::extract::ws::{CloseFrame, Message};
use futures::SinkExt;
use uuid::Uuid;

pub async fn update_game_state(
    new_state: GameState,
    room_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let room_info = match db::room::get_room_info(room_id, redis.clone()).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Failed to fetch room info: {}", e);
            send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
            return;
        }
    };

    if room_info.creator_id != player.id {
        tracing::warn!(
            "Unauthorized game state update attempt by {}",
            player.wallet_address
        );
        send_error_to_player(
            player.id,
            "Only creator can update game state",
            &connections,
            &redis,
        )
        .await;
        return;
    }

    if let Err(e) = db::room::update_game_state(room_id, new_state.clone(), redis.clone()).await {
        tracing::error!("Failed to update game state: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
    } else {
        if new_state == GameState::InProgress {
            let players = db::room::get_room_players(room_id, redis.clone())
                .await
                .unwrap_or_default();
            let not_ready: Vec<_> = players
                .into_iter()
                .filter(|p| p.state != PlayerState::Ready)
                .collect();

            if !not_ready.is_empty() {
                let msg = LobbyServerMessage::PlayersNotReady { players: not_ready };
                send_to_player(player.id, &connections, &msg, &redis).await;

                let game_starting = LobbyServerMessage::GameState {
                    state: new_state,
                    ready_players: None,
                };
                broadcast_to_lobby(room_id, &game_starting, &connections, redis.clone()).await;
                return; // don't start the game
            }

            let redis_clone = redis.clone();
            let conns_clone = connections.clone();
            let player_clone = player.clone();
            tokio::spawn(async move {
                start_countdown(room_id, player_clone, redis_clone, conns_clone).await;
            });

            if let Ok(info) = db::room::get_room_info(room_id, redis.clone()).await {
                if info.state == GameState::InProgress {
                    let game_starting = LobbyServerMessage::GameState {
                        state: new_state,
                        ready_players: None,
                    };
                    broadcast_to_lobby(room_id, &game_starting, &connections, redis.clone()).await;
                } else {
                    tracing::info!(
                        "Game state was reverted before start, skipping GameState message"
                    );
                }
            }
        }
    }
}

async fn close_lobby_connections(player_ids: &[Uuid], connections: &ConnectionInfoMap) {
    let connections_guard = connections.lock().await;

    let mut target_connections = Vec::new();
    for &player_id in player_ids {
        if let Some(connection_info) = connections_guard.get(&player_id) {
            target_connections.push((player_id, connection_info.clone()));
        }
    }

    drop(connections_guard);

    for (player_id, connection_info) in target_connections {
        {
            let mut sender = connection_info.sender.lock().await;
            tracing::info!(
                "Closing lobby connection for player {} (game starting)",
                player_id
            );

            let close_frame = CloseFrame {
                code: axum::extract::ws::close_code::NORMAL,
                reason: "Game starting - redirecting to game".into(),
            };

            let _ = sender.send(Message::Close(Some(close_frame))).await;
        }
    }
}

async fn start_countdown(
    room_id: Uuid,
    player: Player,
    redis: RedisClient,
    connections: ConnectionInfoMap,
) {
    for i in (0..=15).rev() {
        match db::room::get_room_info(room_id, redis.clone()).await {
            Ok(info) => {
                if info.state != GameState::InProgress {
                    tracing::info!("Countdown interrupted by state change");

                    let msg = LobbyServerMessage::GameState {
                        state: info.state,
                        ready_players: None,
                    };
                    broadcast_to_lobby(room_id, &msg, &connections, redis.clone()).await;

                    break;
                }
            }
            Err(e) => {
                tracing::error!("Failed to check state: {}", e);
                send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
                break;
            }
        }

        let countdown_msg = LobbyServerMessage::Countdown { time: i };
        broadcast_to_lobby(room_id, &countdown_msg, &connections, redis.clone()).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Final state confirmation
    if let Ok(info) = db::room::get_room_info(room_id, redis.clone()).await {
        if info.state == GameState::InProgress {
            let ready_players = match db::room::get_ready_room_players(room_id, redis.clone()).await
            {
                Ok(players) => players.into_iter().map(|p| p.id).collect::<Vec<_>>(),
                Err(e) => {
                    tracing::error!("❌ Failed to get ready players: {}", e);
                    send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
                    vec![]
                }
            };

            let players = match db::room::get_room_players(room_id, redis.clone()).await {
                Ok(players) => players,
                Err(e) => {
                    tracing::error!("❌ Failed to get room players: {}", e);
                    send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
                    vec![]
                }
            };

            tracing::info!("Game started with {} ready players", ready_players.len());

            let msg = LobbyServerMessage::GameState {
                state: GameState::InProgress,
                ready_players: Some(ready_players.clone()),
            };
            broadcast_to_lobby(room_id, &msg, &connections, redis.clone()).await;

            if ready_players.len() > 1 {
                // Get all player IDs from the room for disconnection
                let all_player_ids: Vec<Uuid> = players.iter().map(|p| p.id).collect();

                // Remove connections from state
                for player in &players {
                    remove_connection(player.id, &connections).await;
                }

                // Close WebSocket connections with proper close frame
                close_lobby_connections(&all_player_ids, &connections).await;
            }
        }
    }
}
