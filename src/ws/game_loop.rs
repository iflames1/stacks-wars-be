use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::json;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    db::room::update_room_player_after_game,
    models::{GameRoom, RoomPlayer, Standing},
    state::{Connections, RedisClient, Rooms},
    ws::{
        handlers::generate_random_letter,
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

async fn broadcast_to_player(
    target_player_id: Uuid,
    msg_type: &str,
    data: &str,
    connections: &Connections,
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
    connections: &Connections,
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

async fn broadcast_to_room_from_player(
    sender_player: &RoomPlayer,
    msg_type: &str,
    data: &str,
    room: &GameRoom,
    connections: &Connections,
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

fn start_turn_timer(
    player_id: Uuid,
    room_id: Uuid,
    rooms: Rooms,
    connections: Connections,
    words: Arc<HashSet<String>>,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=10).rev() {
            {
                let rooms_guard = rooms.lock().await;
                if let Some(room) = rooms_guard.get(&room_id) {
                    if room.current_turn_id != player_id {
                        broadcast_to_player(player_id, "countdown", "10", &connections).await;
                        println!("turn changed, stopping timer");
                        return;
                    }
                    let countdown = &i.to_string();
                    broadcast_to_player(player_id, "countdown", countdown, &connections).await;
                } else {
                    println!("room not found, stopping timer");
                    return;
                }
            }

            println!("{} seconds left for player {}", i, player_id);
            sleep(Duration::from_secs(1)).await;
        }

        // time ran out
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            if room.current_turn_id == player_id {
                println!("Player {} timed out", player_id);

                let next_player_id = get_next_player_and_wrap(room, player_id);

                // remove player from room and save position
                if let Some(pos) = room.players.iter().position(|p| p.id == player_id) {
                    let player = room.players.remove(pos);
                    room.eliminated_players.push(player.clone());

                    let position = room.players.len() + 1;
                    room.rankings.push((player.id, position)); // TODO: check usage

                    let rank = position.to_string();
                    broadcast_to_player(player_id, "rank", &rank, &connections).await;

                    let player_used_words = room.used_words.remove(&player.id).unwrap_or_default();

                    if let Err(e) = update_room_player_after_game(
                        room_id,
                        player_id,
                        position,
                        player_used_words,
                        redis.clone(),
                    )
                    .await
                    {
                        println!("Error updating player in Redis: {}", e);
                    }
                }

                // check game over
                if room.players.len() == 1 {
                    let winner = room.players.remove(0);
                    room.eliminated_players.push(winner.clone());
                    room.rankings.push((winner.id, 1));

                    broadcast_to_player(winner.id, "rank", "1", &connections).await;

                    let player_used_words = room.used_words.remove(&winner.id).unwrap_or_default();

                    if let Err(e) = update_room_player_after_game(
                        room_id,
                        winner.id,
                        1,
                        player_used_words,
                        redis.clone(),
                    )
                    .await
                    {
                        println!("Error updating player in Redis: {}", e);
                    }

                    let game_over = "🏁 Game Over!".to_string();

                    broadcast_to_room("game_over", &game_over, &room, &connections).await;

                    let standings: Vec<Standing> = room
                        .eliminated_players
                        .iter()
                        .rev()
                        .enumerate()
                        .map(|(index, player)| Standing {
                            wallet_address: player.wallet_address.clone(),
                            rank: index + 1,
                        })
                        .collect();

                    // broadcast final result
                    broadcast_to_room("final_standing", &standings, &room, &connections).await;
                    return;
                }

                if room.players.is_empty() {
                    println!("fix: room {} is now empty", room.info.id); // never really gets here
                    return;
                }

                //continue game if players > 1
                if let Some(next_id) = next_player_id {
                    room.current_turn_id = next_id;

                    if let Some(current_player) = room.players.iter().find(|p| p.id == next_id) {
                        broadcast_to_room(
                            "current_turn",
                            &current_player.wallet_address,
                            &room,
                            &connections,
                        )
                        .await;
                    }

                    start_turn_timer(
                        next_id,
                        room_id,
                        rooms.clone(),
                        connections.clone(),
                        words.clone(),
                        redis.clone(),
                    );
                } else {
                    println!("No next player found in room {}", room.info.id);
                }
            }
        }
    });
}

pub async fn handle_incoming_messages(
    player: &RoomPlayer,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    rooms: Rooms,
    connections: &Connections,
    words: Arc<HashSet<String>>,
    redis: RedisClient,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            println!("Received from {}: {}", player.wallet_address, text);

            let cleaned_word = text.trim().to_lowercase();

            let advance_turn: bool;

            {
                let mut rooms_guard = rooms.lock().await;
                let room = rooms_guard.get_mut(&room_id).unwrap();

                // check turn
                if player.id != room.current_turn_id {
                    println!("Not {}'s turn", player.wallet_address); // broadcast turn to players
                    continue;
                }

                // check if word is used
                if room.used_words_global.contains(&cleaned_word) {
                    println!("This word have been used: {}", cleaned_word); // broadcast to players
                    broadcast_to_player(player.id, "used_word", &cleaned_word, connections).await;
                    continue;
                }

                // apply rule
                if let Some(rule) = get_rule_by_index(room.rule_index, &room.rule_context) {
                    // untested check
                    if rule.name != "min_length" {
                        if cleaned_word.len() < room.rule_context.min_word_length {
                            let reason = format!(
                                "Word must be at least {} characters!",
                                room.rule_context.min_word_length
                            );
                            println!("Rule failed: {}", reason);
                            broadcast_to_player(player.id, "validation_msg", &reason, connections)
                                .await;
                            continue;
                        }
                    }
                    broadcast_to_room("rule", &rule.description, &room, &connections).await;
                    if let Err(reason) = (rule.validate)(&cleaned_word, &room.rule_context) {
                        println!("Rule failed: {}", reason);
                        broadcast_to_player(player.id, "validation_msg", &reason, connections)
                            .await;
                        continue;
                    }
                } else {
                    println!("fix invalid rule index {}", room.rule_index);
                }

                // check if word is valid
                if !words.contains(&cleaned_word) {
                    println!(
                        "invalid word from {}: {}",
                        player.wallet_address, cleaned_word
                    );
                    continue;
                }

                // add to used words
                room.used_words_global.insert(cleaned_word.clone());
                room.used_words
                    .entry(player.id)
                    .or_default()
                    .push(cleaned_word.clone());

                // store next player id
                if let Some(next_id) = get_next_player_and_wrap(room, player.id) {
                    room.current_turn_id = next_id;

                    if let Some(current_player) = room.players.iter().find(|p| p.id == next_id) {
                        broadcast_to_room(
                            "current_turn",
                            &current_player.wallet_address,
                            &room,
                            &connections,
                        )
                        .await;
                    }
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
                    redis.clone(),
                );

                advance_turn = true;
            }

            if advance_turn {
                let room_gaurd = rooms.lock().await;
                let room = room_gaurd.get(&room_id).unwrap();
                broadcast_to_room_from_player(
                    player,
                    "word_entry",
                    &cleaned_word,
                    &room,
                    connections,
                )
                .await;
            }
        }
    }
}
