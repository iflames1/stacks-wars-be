use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    models::{GameRoom, Player},
    state::{Connections, Rooms},
    ws::{
        handler::generate_random_letter,
        rules::{get_rule_by_index, get_rules},
    },
};
use uuid::Uuid;

fn get_next_player_and_wrap(room: &mut GameRoom, current_id: Uuid) -> Option<Uuid> {
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

async fn broadcast_to_player(target_player_id: Uuid, message: &str, connections: &Connections) {
    let connection_guard = connections.lock().await;
    if let Some(sender_arc) = connection_guard.get(&target_player_id) {
        let mut sender = sender_arc.lock().await;
        let _ = sender.send(Message::Text(message.into())).await;
    }
}

async fn broadcast_to_room(message: &str, players: &[Player], connections: &Connections) {
    let connection_guard = connections.lock().await;

    for player in players {
        if let Some(sender_arc) = connection_guard.get(&player.id) {
            let mut sender = sender_arc.lock().await;
            let _ = sender.send(Message::Text(message.to_string().into())).await;
        }
    }
}

async fn broadcast_to_room_from_player(
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
        for i in (0..=10).rev() {
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
            broadcast_to_room_from_player(
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

                //room.players.retain(|p| p.id != player_id);

                if let Some(idx) = current_index {
                    let eliminated_player = room.players.remove(idx);
                    let position = room.players.len() + 1;
                    room.rankings.push((eliminated_player.id, position));

                    if room.players.is_empty() {
                        println!("Room {} is now empty", room.id);
                        return;
                    }

                    // check game over
                    println!("players left: {}", room.players.len());
                    if room.players.len() == 1 {
                        let winner = &room.players[0];
                        room.rankings.push((winner.id, 1));
                        room.game_over = true;

                        // broadcast final result
                        let standing = room
                            .rankings
                            .iter()
                            .map(|(id, pos)| format!("Player {} - {} place", id, pos))
                            .collect::<Vec<_>>()
                            .join("\n");

                        broadcast_to_room("Game Over!", &room.players, &connections).await;

                        broadcast_to_room(
                            &format!("Final standing: {}", standing),
                            &room.players,
                            &connections,
                        )
                        .await;

                        broadcast_to_player(
                            winner.id,
                            &format!("🏆 You're the Winner: {}.", winner.id),
                            &connections,
                        )
                        .await;

                        return;
                    }

                    let current_player_id = if idx >= room.players.len() {
                        room.players[0].id
                    } else {
                        room.players[idx].id
                    };

                    if let Some(next_id) = get_next_player_and_wrap(room, current_player_id) {
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

                // check if word is used
                if room.used_words.contains(&cleaned_word) {
                    println!("This word have been used: {}", cleaned_word);
                    continue;
                }

                // apply rule
                if let Some(rule) = get_rule_by_index(room.rule_index, &room.rule_context) {
                    if let Err(reason) = (rule.validate)(&cleaned_word, &room.rule_context) {
                        println!("Rule failed: {}", reason);
                        broadcast_to_player(player.id, &reason, connections).await;
                        continue;
                    }
                } else {
                    println!("fix nvalid rule index {}", room.rule_index);
                }

                // check if word is valid
                if !words.contains(&cleaned_word) {
                    println!("invalid word from {}: {}", player.id, cleaned_word);
                    continue;
                }

                // add to used words
                room.used_words.insert(cleaned_word.clone());

                // store next player id
                if let Some(next_id) = get_next_player_and_wrap(room, player.id) {
                    room.current_turn_id = next_id;
                } else {
                    println!("couldn't find next player");
                };

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
                broadcast_to_room_from_player(player, &cleaned_word, room_id, &rooms, connections)
                    .await;
            }
        }
    }
}
