use crate::{
    db::lobby::{
        get::{get_lobby_info, get_lobby_players},
        patch::update_lobby_state,
    },
    models::{
        game::{LobbyState, Player, PlayerState},
        lobby::LobbyServerMessage,
    },
    state::{ConnectionInfoMap, CountdownState, LobbyCountdowns, RedisClient},
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
    new_state: LobbyState,
    lobby_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
    countdowns: &LobbyCountdowns,
) {
    let lobby_info = match get_lobby_info(lobby_id, redis.clone()).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Failed to fetch lobby info: {}", e);
            send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis).await;
            return;
        }
    };

    if lobby_info.creator.id != player.id {
        tracing::warn!("Unauthorized game state update attempt by {}", player.id);
        send_error_to_player(
            player.id,
            lobby_id,
            "Only creator can update game state",
            &connections,
            &redis,
        )
        .await;
        return;
    }

    if let Err(e) = update_lobby_state(lobby_id, new_state.clone(), redis.clone()).await {
        tracing::error!("Failed to update game state: {}", e);
        send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis).await;
    } else {
        if new_state == LobbyState::InProgress {
            let players = get_lobby_players(lobby_id, None, redis.clone())
                .await
                .unwrap_or_default();
            let not_ready: Vec<_> = players
                .into_iter()
                .filter(|p| p.state != PlayerState::Ready)
                .collect();

            if !not_ready.is_empty() {
                let msg = LobbyServerMessage::PlayersNotReady { players: not_ready };
                send_to_player(player.id, lobby_id, &connections, &msg, &redis).await;

                let game_starting = LobbyServerMessage::LobbyState {
                    state: new_state,
                    ready_players: None,
                };
                broadcast_to_lobby(lobby_id, &game_starting, &connections, None, redis.clone())
                    .await;
                return; // don't start the game
            }

            let redis_clone = redis.clone();
            let conns_clone = connections.clone();
            let countdowns_clone = countdowns.clone();
            let player_clone = player.clone();
            tokio::spawn(async move {
                start_countdown(
                    lobby_id,
                    player_clone,
                    redis_clone,
                    conns_clone,
                    countdowns_clone,
                )
                .await;
            });

            if let Ok(info) = get_lobby_info(lobby_id, redis.clone()).await {
                if info.state == LobbyState::InProgress {
                    let game_starting = LobbyServerMessage::LobbyState {
                        state: new_state,
                        ready_players: None,
                    };
                    broadcast_to_lobby(lobby_id, &game_starting, &connections, None, redis.clone())
                        .await;
                } else {
                    tracing::info!(
                        "Game state was reverted before start, skipping LobbyState message"
                    );
                }
            }
        } else {
            // If game state is not InProgress, clear any existing countdown
            {
                let mut countdowns_guard = countdowns.lock().await;
                countdowns_guard.remove(&lobby_id);
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
    lobby_id: Uuid,
    player: Player,
    redis: RedisClient,
    connections: ConnectionInfoMap,
    countdowns: LobbyCountdowns,
) {
    // Initialize countdown state
    {
        let mut countdowns_guard = countdowns.lock().await;
        countdowns_guard.insert(lobby_id, CountdownState { current_time: 15 });
    }

    for i in (0..=15).rev() {
        // Update countdown state
        {
            let mut countdowns_guard = countdowns.lock().await;
            if let Some(countdown_state) = countdowns_guard.get_mut(&lobby_id) {
                countdown_state.current_time = i;
            }
        }

        match get_lobby_info(lobby_id, redis.clone()).await {
            Ok(info) => {
                if info.state != LobbyState::InProgress {
                    tracing::info!("Countdown interrupted by state change");

                    // Clear countdown state
                    {
                        let mut countdowns_guard = countdowns.lock().await;
                        countdowns_guard.remove(&lobby_id);
                    }

                    let msg = LobbyServerMessage::LobbyState {
                        state: info.state,
                        ready_players: None,
                    };
                    broadcast_to_lobby(lobby_id, &msg, &connections, None, redis.clone()).await;

                    break;
                }
            }
            Err(e) => {
                tracing::error!("Failed to check state: {}", e);
                send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis)
                    .await;

                // Clear countdown state on error
                {
                    let mut countdowns_guard = countdowns.lock().await;
                    countdowns_guard.remove(&lobby_id);
                }
                break;
            }
        }

        let countdown_msg = LobbyServerMessage::Countdown { time: i };
        broadcast_to_lobby(lobby_id, &countdown_msg, &connections, None, redis.clone()).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Final state confirmation
    if let Ok(info) = get_lobby_info(lobby_id, redis.clone()).await {
        if info.state == LobbyState::InProgress {
            let ready_players =
                match get_lobby_players(lobby_id, Some(PlayerState::Ready), redis.clone()).await {
                    Ok(players) => players.into_iter().map(|p| p.id).collect::<Vec<_>>(),
                    Err(e) => {
                        tracing::error!("❌ Failed to get ready players: {}", e);
                        send_error_to_player(
                            player.id,
                            lobby_id,
                            e.to_string(),
                            &connections,
                            &redis,
                        )
                        .await;
                        vec![]
                    }
                };

            let players = match get_lobby_players(lobby_id, None, redis.clone()).await {
                Ok(players) => players,
                Err(e) => {
                    tracing::error!("❌ Failed to get lobby players: {}", e);
                    send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis)
                        .await;
                    vec![]
                }
            };

            tracing::info!("Game started with {} ready players", ready_players.len());

            let msg = LobbyServerMessage::LobbyState {
                state: LobbyState::InProgress,
                ready_players: Some(ready_players.clone()),
            };
            broadcast_to_lobby(lobby_id, &msg, &connections, None, redis.clone()).await;

            // Clear countdown state since game has officially started
            {
                let mut countdowns_guard = countdowns.lock().await;
                countdowns_guard.remove(&lobby_id);
            }

            if ready_players.len() > 1 {
                // Get all player IDs from the lobby for disconnection
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
