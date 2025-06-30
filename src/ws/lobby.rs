use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::{
    db,
    models::game::{GameState, LobbyClientMessage, LobbyServerMessage, Player},
    state::{PlayerConnections, RedisClient},
    ws::handlers::remove_connection,
};

pub async fn broadcast_to_lobby(
    room_id: Uuid,
    msg: &LobbyServerMessage,
    connections: &PlayerConnections,
    redis: RedisClient,
) {
    let serialized = match serde_json::to_string(msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize message: {}", e);
            return;
        }
    };

    if let Ok(players) = db::room::get_room_players(room_id, redis).await {
        let connection_guard = connections.lock().await;

        for player in players {
            if let Some(sender_arc) = connection_guard.get(&player.id) {
                let mut sender = sender_arc.lock().await;
                let _ = sender.send(Message::Text(serialized.clone().into())).await;
            }
        }
    }
}

pub async fn handle_incoming_messages(
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    room_id: Uuid,
    player: &Player,
    connections: &PlayerConnections,
    redis: RedisClient,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<LobbyClientMessage>(&text) {
                    match parsed {
                        LobbyClientMessage::UpdatePlayerState { new_state } => {
                            if let Err(e) = db::room::update_player_state(
                                room_id,
                                player.id,
                                new_state,
                                redis.clone(),
                            )
                            .await
                            {
                                tracing::error!("Failed to update state: {}", e);
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerUpdated { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                        }
                        LobbyClientMessage::LeaveRoom => {
                            if let Err(e) =
                                db::room::leave_room(room_id, player.id, redis.clone()).await
                            {
                                tracing::error!("Failed to leave room: {}", e);
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerLeft { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                            remove_connection(player.id, &connections).await;
                            break;
                        }
                        LobbyClientMessage::KickPlayer {
                            player_id,
                            wallet_address,
                            display_name,
                        } => {
                            let room_info =
                                match db::room::get_room_info(room_id, redis.clone()).await {
                                    Ok(info) => info,
                                    Err(e) => {
                                        tracing::error!("Failed to fetch room info: {}", e);
                                        continue;
                                    }
                                };

                            if room_info.creator_id != player.id {
                                tracing::error!(
                                    "Unauthorized kick attempt by {}",
                                    player.wallet_address
                                );
                                continue;
                            }

                            if room_info.state != GameState::Waiting {
                                tracing::error!("Cannot kick players when game is not waiting");
                                continue;
                            }

                            // Remove player
                            if let Err(e) =
                                db::room::leave_room(room_id, player_id, redis.clone()).await
                            {
                                tracing::error!("Failed to kick player: {}", e);
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerLeft { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;

                                let kicked_msg = LobbyServerMessage::PlayerKicked {
                                    player_id,
                                    wallet_address,
                                    display_name,
                                };
                                broadcast_to_lobby(
                                    room_id,
                                    &kicked_msg,
                                    &connections,
                                    redis.clone(),
                                )
                                .await;

                                let notify_msg: LobbyServerMessage =
                                    LobbyServerMessage::NotifyKicked;

                                let conns = connections.lock().await;
                                if let Some(sender) = conns.get(&player_id) {
                                    let mut sender = sender.lock().await;
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(&notify_msg).unwrap().into(),
                                        ))
                                        .await;
                                    let _ = sender.send(Message::Close(None)).await;
                                }
                            }

                            // Optionally: disconnect the kicked player
                            let conns = connections.lock().await;
                            if let Some(sender) = conns.get(&player_id) {
                                let mut sender = sender.lock().await;
                                let _ = sender.send(Message::Close(None)).await;
                            }
                        }
                        LobbyClientMessage::UpdateGameState { new_state } => {
                            println!("called with {new_state:?}");
                            let room_info =
                                match db::room::get_room_info(room_id, redis.clone()).await {
                                    Ok(info) => info,
                                    Err(e) => {
                                        eprintln!("Failed to fetch room info: {}", e);
                                        continue;
                                    }
                                };

                            if room_info.creator_id != player.id {
                                eprintln!(
                                    "Unauthorized game state update attempt by {}",
                                    player.wallet_address
                                );
                                continue;
                            }

                            if let Err(e) = db::room::update_game_state(
                                room_id,
                                new_state.clone(),
                                redis.clone(),
                            )
                            .await
                            {
                                eprintln!("Failed to update game state: {}", e);
                            } else {
                                if new_state == GameState::InProgress {
                                    let redis_clone = redis.clone();
                                    let conns_clone = connections.clone();
                                    tokio::spawn(async move {
                                        start_countdown(room_id, redis_clone, conns_clone).await;
                                    });

                                    if let Ok(info) =
                                        db::room::get_room_info(room_id, redis.clone()).await
                                    {
                                        if info.state == GameState::InProgress {
                                            let game_starting = LobbyServerMessage::GameState {
                                                state: new_state,
                                                ready_players: None,
                                            };
                                            broadcast_to_lobby(
                                                room_id,
                                                &game_starting,
                                                &connections,
                                                redis.clone(),
                                            )
                                            .await;
                                        } else {
                                            tracing::info!(
                                                "Game state was reverted before start, skipping GameState message"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

async fn start_countdown(room_id: Uuid, redis: RedisClient, connections: PlayerConnections) {
    for i in (0..=30).rev() {
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
                    vec![]
                }
            };

            let players = match db::room::get_room_players(room_id, redis.clone()).await {
                Ok(players) => players,
                Err(e) => {
                    tracing::error!("❌ Failed to get room players: {}", e);
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
                for player in players {
                    remove_connection(player.id, &connections).await;
                }
            }
        }
    }
}
