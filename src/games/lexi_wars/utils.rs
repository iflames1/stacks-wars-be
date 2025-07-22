use axum::extract::ws::Message;
use futures::SinkExt;
use rand::{Rng, rng};

use crate::{
    games::lexi_wars::rules::get_rules,
    models::{
        game::{GameRoom, Player},
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

pub fn get_next_player_and_wrap(room: &mut GameRoom, current_id: Uuid) -> Option<Uuid> {
    // Use connected_players instead of all players
    let connected_players = &room.connected_players;

    connected_players
        .iter()
        .position(|p| p.id == current_id)
        .map(|i| {
            let next_index = (i + 1) % connected_players.len();
            let next_id = connected_players[next_index].id;
            let wrapped = next_index == 0;

            if wrapped {
                let next_rule_index = (room.rule_index + 1) % get_rules(&room.rule_context).len();

                // If we wrapped to first rule again, increase difficulty
                if next_rule_index == 0 {
                    room.rule_context.min_word_length += 2;
                }

                room.rule_index = next_rule_index;
                room.rule_context.random_letter = generate_random_letter();
            }

            next_id
        })
}

pub async fn broadcast_to_player(
    target_player_id: Uuid,
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

            // Queue the message when send fails
            drop(sender);
            drop(connection_guard);

            if let Err(queue_err) =
                queue_message_for_player(target_player_id, serialized, redis).await
            {
                tracing::error!(
                    "Failed to queue message for player {}: {}",
                    target_player_id,
                    queue_err
                );
            }
        }
    } else {
        // Player not connected, queue the message
        if let Err(e) = queue_message_for_player(target_player_id, serialized, redis).await {
            tracing::error!(
                "Failed to queue message for offline player {}: {}",
                target_player_id,
                e
            );
        }
    }
}

pub async fn broadcast_to_room(
    message: &LexiWarsServerMessage,
    room: &GameRoom,
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

    // Use connected_players instead of all players
    for player in room
        .connected_players
        .iter()
        .chain(room.eliminated_players.iter())
    {
        if let Some(conn_info) = connection_guard.get(&player.id) {
            let mut sender = conn_info.sender.lock().await;
            if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
                tracing::warn!("Failed to send message to player {}: {}", player.id, e);

                // Queue the message when send fails
                drop(sender);

                if let Err(queue_err) =
                    queue_message_for_player(player.id, serialized.clone(), redis).await
                {
                    tracing::error!(
                        "Failed to queue message for player {}: {}",
                        player.id,
                        queue_err
                    );
                }
            }
        } else {
            // Player not connected, queue the message
            if let Err(e) = queue_message_for_player(player.id, serialized.clone(), redis).await {
                tracing::error!(
                    "Failed to queue message for offline player {}: {}",
                    player.id,
                    e
                );
            }
        }
    }
}

pub async fn broadcast_word_entry_from_player(
    sender_player: &Player,
    word: &str,
    room: &GameRoom,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let message = LexiWarsServerMessage::WordEntry {
        word: word.to_string(),
        sender: sender_player.clone(),
    };

    broadcast_to_room(&message, room, connections, redis).await;
}
