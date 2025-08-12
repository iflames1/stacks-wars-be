use axum::extract::ws::{CloseFrame, Message};
use futures::SinkExt;
use rand::{Rng, rng};

use crate::{
    games::lexi_wars::rules::get_rules,
    models::{
        game::{LexiWars, Player},
        lexi_wars::LexiWarsServerMessage,
    },
    state::{ConnectionInfoMap, RedisClient},
    ws::handlers::utils::queue_message_for_player,
};
use uuid::Uuid;

pub fn generate_random_letter() -> char {
    let letter = rng().random_range(0..26);
    (b'a' + letter as u8) as char
}

pub fn get_next_player_id_and_wrap(lobby: &LexiWars, current_player_id: Uuid) -> Option<Uuid> {
    if let Some(current_index) = lobby
        .connected_player_ids
        .iter()
        .position(|&id| id == current_player_id)
    {
        let next_index = (current_index + 1) % lobby.connected_player_ids.len();
        lobby.connected_player_ids.get(next_index).copied()
    } else {
        lobby.connected_player_ids.first().copied()
    }
}

pub fn get_next_player_and_wrap(lobby: &mut LexiWars, current_id: Uuid) -> Option<Uuid> {
    // Use connected_player_ids instead of connected_players
    let connected_player_ids = &lobby.connected_player_ids;

    connected_player_ids
        .iter()
        .position(|&id| id == current_id) // Fixed: compare UUIDs directly
        .map(|i| {
            let next_index = (i + 1) % connected_player_ids.len();
            let next_id = connected_player_ids[next_index]; // Fixed: get UUID, not .id
            let wrapped = next_index == 0;

            if wrapped {
                let next_rule_index = (lobby.rule_index + 1) % get_rules(&lobby.rule_context).len();

                // If we wrapped to first rule again, increase difficulty
                if next_rule_index == 0 {
                    lobby.rule_context.min_word_length += 2;
                }

                lobby.rule_index = next_rule_index;
                lobby.rule_context.random_letter = generate_random_letter();
            }

            next_id
        })
}

pub async fn broadcast_to_player(
    target_player_id: Uuid,
    lobby_id: Uuid,
    message: &LexiWarsServerMessage,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let serialized = match serde_json::to_string(message) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize LexiWarsServerMessage: {}", e);
            return;
        }
    };

    let connection_guard = connections.lock().await;
    if let Some(conn_info) = connection_guard.get(&target_player_id) {
        let mut sender = conn_info.sender.lock().await;
        if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
            tracing::warn!(
                "Failed to send message to player {}: {}",
                target_player_id,
                e
            );

            if message.should_queue() {
                drop(sender);
                drop(connection_guard);

                if let Err(queue_err) =
                    queue_message_for_player(target_player_id, lobby_id, serialized, redis).await
                {
                    tracing::error!(
                        "Failed to queue message for player {}: {}",
                        target_player_id,
                        queue_err
                    );
                }
            }
        }
    } else {
        if message.should_queue() {
            if let Err(e) =
                queue_message_for_player(target_player_id, lobby_id, serialized, redis).await
            {
                tracing::error!(
                    "Failed to queue message for offline player {}: {}",
                    target_player_id,
                    e
                );
            }
        }
    }
}

pub async fn broadcast_to_lobby(
    message: &LexiWarsServerMessage,
    lobby: &LexiWars,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let serialized = match serde_json::to_string(message) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize LexiWarsServerMessage: {}", e);
            return;
        }
    };

    let connection_guard = connections.lock().await;

    let all_relevant_players: Vec<&Player> = lobby
        .players
        .iter()
        .filter(|p| {
            lobby.connected_player_ids.contains(&p.id)
                || lobby.eliminated_players.iter().any(|ep| ep.id == p.id)
        })
        .collect();

    for player in all_relevant_players {
        if let Some(conn_info) = connection_guard.get(&player.id) {
            let mut sender = conn_info.sender.lock().await;
            if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
                tracing::warn!("Failed to send message to player {}: {}", player.id, e);

                // Only queue the message if it should be queued
                if message.should_queue() {
                    drop(sender);

                    if let Err(queue_err) = queue_message_for_player(
                        player.id,
                        lobby.info.id,
                        serialized.clone(),
                        redis,
                    )
                    .await
                    {
                        tracing::error!(
                            "Failed to queue message for player {}: {}",
                            player.id,
                            queue_err
                        );
                    }
                }
            }
        } else {
            // Player not connected, only queue if message should be queued
            if message.should_queue() {
                if let Err(e) =
                    queue_message_for_player(player.id, lobby.info.id, serialized.clone(), redis)
                        .await
                {
                    tracing::error!(
                        "Failed to queue message for offline player {}: {}",
                        player.id,
                        e
                    );
                }
            }
        }
    }
}

pub async fn broadcast_word_entry_from_player(
    sender_player: &Player,
    word: &str,
    lobby: &LexiWars,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let message = LexiWarsServerMessage::WordEntry {
        word: word.to_string(),
        sender: sender_player.clone(),
    };

    broadcast_to_lobby(&message, lobby, connections, redis).await;
}

pub async fn close_connections_for_players(player_ids: &[Uuid], connections: &ConnectionInfoMap) {
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
                "Closing connection for player {} (game finished)",
                player_id
            );

            let close_frame = CloseFrame {
                code: axum::extract::ws::close_code::NORMAL,
                reason: "Game finished".into(),
            };

            let _ = sender.send(Message::Close(Some(close_frame))).await;
        }
    }
}
