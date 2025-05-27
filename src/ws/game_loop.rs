use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    models::Player,
    state::{Connections, Rooms},
};
use uuid::Uuid;

fn next_turn(players: &[Player], current_id: Uuid) -> Option<Uuid> {
    players.iter().position(|p| p.id == current_id).map(|i| {
        let next_index = (i + 1) % players.len(); // wrap around
        players[next_index].id
    })
}

async fn broadcast_to_room(
    sender_player: &Player,
    message: &str,
    room_id: Uuid,
    rooms: &Rooms,
    connections: &Connections,
) {
    let rooms_guard = rooms.lock().await;
    if let Some(room) = rooms_guard.get(&room_id) {
        let connection_guard = connections.lock().await;
        for p in &room.players {
            if let Some(sender_arc) = connection_guard.get(&p.id) {
                let mut sender = sender_arc.lock().await;
                let _ = sender
                    .send(Message::Text(
                        format!("{}: {}", sender_player.id, message).into(),
                    ))
                    .await;
            }
        }
    }
}

fn start_turn_timer(
    player_id: Uuid,
    room_id: Uuid,
    rooms: Rooms,
    connections: Connections,
    words: Arc<HashSet<String>>,
) {
    tokio::spawn(async move {
        for i in (1..=10).rev() {
            {
                let rooms_guard = rooms.lock().await;
                if let Some(room) = rooms_guard.get(&room_id) {
                    if room.current_turn_id != player_id {
                        println!("turn changed, stopping timer");
                        return;
                    }
                } else {
                    println!("room not found, stopping timer");
                    return;
                }
            }

            let countdown_msg = format!("{} seconds left", i);
            broadcast_to_room(
                &Player {
                    id: player_id,
                    username: None,
                },
                &countdown_msg,
                room_id,
                &rooms,
                &connections,
            )
            .await;
            println!("{} seconds left for player {}", i, player_id);
            sleep(Duration::from_secs(1)).await;
        }

        // time ran out
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            if room.current_turn_id == player_id {
                println!("Player {} timed out", player_id);

                // Find index BEFORE removing the player
                let current_index = room.players.iter().position(|p| p.id == player_id);

                room.players.retain(|p| p.id != player_id);

                if room.players.is_empty() {
                    println!("Room {} is now empty", room.id);
                    return;
                }

                if let Some(idx) = current_index {
                    let next_index = if idx >= room.players.len() {
                        0 // wrap around if needed
                    } else {
                        idx
                    };
                    let next_id = room.players[next_index].id;
                    room.current_turn_id = next_id;

                    start_turn_timer(
                        next_id,
                        room_id,
                        rooms.clone(),
                        connections.clone(),
                        words.clone(),
                    );
                } else {
                    println!("Couldn't find timed-out player index in room {}", room.id);
                }
            }
        }
    });
}

pub async fn handle_incoming_messages(
    player: &Player,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    rooms: Rooms,
    connections: &Connections,
    words: Arc<HashSet<String>>,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            println!("Received from {}: {}", player.id, text);

            let cleaned_word = text.trim().to_lowercase();

            let advance_turn: bool;

            {
                let mut rooms_guard = rooms.lock().await;
                let room = rooms_guard.get_mut(&room_id).unwrap();

                // check turn
                if player.id != room.current_turn_id {
                    println!("Not {}'s turn", player.id);
                    continue;
                }

                // check if word is valid
                if !words.contains(&cleaned_word) {
                    println!("invalid word from {}: {}", player.id, cleaned_word);
                    continue;
                }

                // check if word is used
                if room.used_words.contains(&cleaned_word) {
                    println!("This word have been used: {}", cleaned_word);
                    continue;
                }

                // add to used words
                room.used_words.insert(cleaned_word.clone());

                // store next player id
                if let Some(next_id) = next_turn(&room.players, player.id) {
                    room.current_turn_id = next_id;
                }

                // start game loop
                start_turn_timer(
                    room.current_turn_id,
                    room_id,
                    rooms.clone(),
                    connections.clone(),
                    words.clone(),
                );

                advance_turn = true;
            }

            if advance_turn {
                broadcast_to_room(player, &cleaned_word, room_id, &rooms, connections).await;
            }
        }
    }
}
