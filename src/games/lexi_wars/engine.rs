use axum::extract::ws::Message;
use futures::StreamExt;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    db::{room::update_room_player_after_game, update_game_state},
    games::lexi_wars::{
        rules::get_rule_by_index,
        utils::{
            broadcast_to_player, broadcast_to_room, broadcast_to_room_from_player,
            get_next_player_and_wrap,
        },
    },
    models::{GameData, GameState, Player, Standing},
    state::{Connections, RedisClient, Rooms},
};
use uuid::Uuid;

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

            //println!("{} seconds left for player {}", i, player_id);
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

                    if let Err(e) =
                        update_game_state(room_id, GameState::Finished, redis.clone()).await
                    {
                        println!("Error updating game state in Redis: {}", e);
                    }

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
    player: &Player,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    rooms: Rooms,
    connections: &Connections,
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
                let words = match &room.data {
                    GameData::LexiWar { word_list } => word_list.clone(),
                };

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
