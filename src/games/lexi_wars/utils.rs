use axum::extract::ws::Message;
use futures::SinkExt;
use rand::{Rng, rng};
use serde::Serialize;
use serde_json::json;

use crate::{
    games::lexi_wars::rules::get_rules,
    models::game::{GameRoom, Player},
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
    msg_type: &str,
    data: &str,
    connections: &PlayerConnections,
) {
    let connection_guard = connections.lock().await;
    if let Some(sender_arc) = connection_guard.get(&target_player_id) {
        let message = json!({
            "type": msg_type,
            "data": data,
        })
        .to_string();
        let mut sender = sender_arc.lock().await;
        let _ = sender.send(Message::Text(message.into())).await;
    }
}

pub async fn broadcast_to_room<T: Serialize>(
    msg_type: &str,
    data: &T,
    room: &GameRoom,
    connections: &PlayerConnections,
) {
    let connection_guard = connections.lock().await;

    let message = json!({
        "type": msg_type,
        "data": data,
    });

    for player in room.players.iter().chain(room.eliminated_players.iter()) {
        if let Some(sender_arc) = connection_guard.get(&player.id) {
            let mut sender = sender_arc.lock().await;
            let _ = sender.send(Message::Text(message.to_string().into())).await;
        }
    }
}

pub async fn broadcast_to_room_from_player(
    sender_player: &Player,
    msg_type: &str,
    data: &str,
    room: &GameRoom,
    connections: &PlayerConnections,
) {
    let connection_guard = connections.lock().await;

    let message = json!({
        "type": msg_type,
        "data": data,
        "sender": sender_player.wallet_address,
    });

    for player in &room.players {
        if let Some(sender_arc) = connection_guard.get(&player.id) {
            let mut sender = sender_arc.lock().await;
            let _ = sender.send(Message::Text(message.to_string().into())).await;
        }
    }
}
