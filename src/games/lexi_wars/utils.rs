use axum::extract::ws::Message;
use futures::SinkExt;
use rand::{Rng, rng};

use crate::{
    games::lexi_wars::rules::get_rules,
    models::game::{GameRoom, LexiWarsServerMessage, Player},
    state::PlayerConnections,
};
use uuid::Uuid;

pub fn generate_random_letter() -> char {
    let letter = rng().random_range(0..26);
    (b'a' + letter as u8) as char
}

pub fn get_next_player_and_wrap(room: &mut GameRoom, current_id: Uuid) -> Option<Uuid> {
    let players = &room.players;

    players.iter().position(|p| p.id == current_id).map(|i| {
        let next_index = (i + 1) % players.len();
        let next_id = players[next_index].id;
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
    connections: &PlayerConnections,
) {
    let connection_guard = connections.lock().await;
    if let Some(sender_arc) = connection_guard.get(&target_player_id) {
        if let Ok(serialized) = serde_json::to_string(message) {
            let mut sender = sender_arc.lock().await;
            let _ = sender.send(Message::Text(serialized.into())).await;
        } else {
            tracing::error!("Failed to serialize LexiWarsServerMessage");
        }
    }
}

pub async fn broadcast_to_room(
    message: &LexiWarsServerMessage,
    room: &GameRoom,
    connections: &PlayerConnections,
) {
    let connection_guard = connections.lock().await;

    if let Ok(serialized) = serde_json::to_string(message) {
        for player in room.players.iter().chain(room.eliminated_players.iter()) {
            if let Some(sender_arc) = connection_guard.get(&player.id) {
                let mut sender = sender_arc.lock().await;
                let _ = sender.send(Message::Text(serialized.clone().into())).await;
            }
        }
    } else {
        tracing::error!("Failed to serialize LexiWarsServerMessage");
    }
}

pub async fn broadcast_word_entry_from_player(
    sender_player: &Player,
    word: &str,
    room: &GameRoom,
    connections: &PlayerConnections,
) {
    let message = LexiWarsServerMessage::WordEntry {
        word: word.to_string(),
        sender: sender_player.clone(),
    };

    broadcast_to_room(&message, room, connections).await;
}
