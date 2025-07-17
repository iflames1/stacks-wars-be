use axum::extract::ws::Message;
use futures::StreamExt;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    db::{room::update_room_player_after_game, update_game_state},
    games::lexi_wars::{
        rules::get_rule_by_index,
        utils::{
            broadcast_to_player, broadcast_to_room, broadcast_word_entry_from_player,
            get_next_player_and_wrap,
        },
    },
    models::game::{
        GameData, GameRoom, GameState, LexiWarsClientMessage, LexiWarsServerMessage, Player,
        PlayerStanding,
    },
    state::{PlayerConnections, RedisClient, SharedRooms},
};
use uuid::Uuid;

fn get_prize(room: &mut GameRoom, position: usize) -> Option<f64> {
    let prize = room.pool.as_ref().map(|pool| match position {
        1 => {
            if room.eliminated_players.len() == 2 {
                (pool.current_amount * 70.0) / 100.0
            } else {
                (pool.current_amount * 50.0) / 100.0
            }
        }
        2 => (pool.current_amount * 30.0) / 100.0,
        3 => (pool.current_amount * 20.0) / 100.0,
        _ => 0.0,
    });

    prize
}

fn start_turn_timer(
    player_id: Uuid,
    room_id: Uuid,
    rooms: SharedRooms,
    connections: PlayerConnections,
    words: Arc<HashSet<String>>,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=30).rev() {
            {
                let rooms_guard = rooms.lock().await;
                if let Some(room) = rooms_guard.get(&room_id) {
                    if room.current_turn_id != player_id {
                        let countdown_msg = LexiWarsServerMessage::Countdown { time: 10 };
                        broadcast_to_player(player_id, &countdown_msg, &connections).await;

                        tracing::info!("turn changed, stopping timer");
                        return;
                    }
                    let countdown_msg = LexiWarsServerMessage::Countdown { time: i };
                    broadcast_to_player(player_id, &countdown_msg, &connections).await;
                } else {
                    tracing::error!("room not found, stopping timer");
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // time ran out
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            if room.current_turn_id == player_id {
                tracing::info!("Player {} timed out", player_id);

                let next_player_id = get_next_player_and_wrap(room, player_id);

                // remove player from room and save position
                if let Some(pos) = room.players.iter().position(|p| p.id == player_id) {
                    let player = room.players.remove(pos);
                    room.eliminated_players.push(player.clone());

                    let position = room.players.len() + 1;

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: position.to_string(),
                    };
                    broadcast_to_player(player_id, &rank_msg, &connections).await;

                    let player_used_words = room.used_words.remove(&player.id).unwrap_or_default();

                    let prize = get_prize(room, position);

                    if let Err(e) = update_room_player_after_game(
                        room_id,
                        player_id,
                        position,
                        prize,
                        player_used_words,
                        redis.clone(),
                    )
                    .await
                    {
                        tracing::error!("Error updating player in Redis: {}", e);
                    }

                    if let Some(amount) = prize {
                        if amount > 0.0 {
                            let prize_msg = LexiWarsServerMessage::Prize { amount };
                            broadcast_to_player(player_id, &prize_msg, &connections).await;
                        }
                    }
                }

                // check game over
                if room.players.len() == 1 {
                    let winner = room.players.remove(0);
                    room.eliminated_players.push(winner.clone());

                    let position = room.players.len() + 1;

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: position.to_string(),
                    };
                    broadcast_to_player(player_id, &rank_msg, &connections).await;

                    let player_used_words = room.used_words.remove(&winner.id).unwrap_or_default();

                    let prize = get_prize(room, position);

                    if let Err(e) = update_room_player_after_game(
                        room_id,
                        winner.id,
                        1,
                        prize,
                        player_used_words,
                        redis.clone(),
                    )
                    .await
                    {
                        tracing::error!("Error updating player in Redis: {}", e);
                    }

                    if let Some(amount) = prize {
                        if amount > 0.0 {
                            let prize_msg = LexiWarsServerMessage::Prize { amount };
                            broadcast_to_player(player_id, &prize_msg, &connections).await;
                        }
                    }

                    let gameover_msg = LexiWarsServerMessage::GameOver;
                    broadcast_to_room(&gameover_msg, &room, &connections).await;

                    let standing: Vec<PlayerStanding> = room
                        .eliminated_players
                        .iter()
                        .rev()
                        .enumerate()
                        .map(|(index, player)| PlayerStanding {
                            player: player.clone(),
                            rank: index + 1,
                        })
                        .collect();

                    let final_standing_msg = LexiWarsServerMessage::FinalStanding { standing };
                    broadcast_to_room(&final_standing_msg, &room, &connections).await;

                    if let Err(e) =
                        update_game_state(room_id, GameState::Finished, redis.clone()).await
                    {
                        tracing::error!("Error updating game state in Redis: {}", e);
                    }

                    return;
                }

                if room.players.is_empty() {
                    tracing::warn!("fix: room {} is now empty", room.info.id); // never really gets here
                    return;
                }

                //continue game if players > 1
                if let Some(next_id) = next_player_id {
                    room.current_turn_id = next_id;

                    if let Some(current_player) = room.players.iter().find(|p| p.id == next_id) {
                        let next_turn_msg = LexiWarsServerMessage::Turn {
                            current_turn: current_player.clone(),
                        };
                        broadcast_to_room(&next_turn_msg, &room, &connections).await;
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
                    tracing::warn!("No next player found in room {}", room.info.id);
                }
            }
        }
    });
}

pub async fn handle_incoming_messages(
    player: &Player,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    rooms: SharedRooms,
    connections: &PlayerConnections,
    redis: RedisClient,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            tracing::info!("Received from {}: {}", player.wallet_address, text);

            let parsed = match serde_json::from_str::<LexiWarsClientMessage>(&text) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::info!("Invalid message format: {}", e);
                    continue;
                }
            };

            match parsed {
                LexiWarsClientMessage::WordEntry { word } => {
                    let cleaned_word = word.trim().to_lowercase();
                    // now continue the original logic with `cleaned_word`
                    let advance_turn: bool;

                    {
                        let mut rooms_guard = rooms.lock().await;
                        let room = rooms_guard.get_mut(&room_id).unwrap();
                        let words = match &room.data {
                            GameData::LexiWar { word_list } => word_list.clone(),
                        };

                        // check turn
                        if player.id != room.current_turn_id {
                            tracing::info!("Not {}'s turn", player.wallet_address); // broadcast turn to players
                            continue;
                        }

                        // check if word is used
                        if room.used_words_global.contains(&cleaned_word) {
                            tracing::info!("This word have been used: {}", cleaned_word);
                            let used_word_msg = LexiWarsServerMessage::UsedWord {
                                word: cleaned_word.clone(),
                            };
                            broadcast_to_player(player.id, &used_word_msg, connections).await;
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
                                    tracing::info!("Rule failed: {}", reason);
                                    let validation_msg =
                                        LexiWarsServerMessage::Validate { msg: reason };
                                    broadcast_to_player(player.id, &validation_msg, connections)
                                        .await;
                                    continue;
                                }
                            }
                            let rule_msg = LexiWarsServerMessage::Rule {
                                rule: rule.description,
                            };
                            broadcast_to_room(&rule_msg, &room, &connections).await;
                            if let Err(reason) = (rule.validate)(&cleaned_word, &room.rule_context)
                            {
                                tracing::info!("Rule failed: {}", reason);
                                let validation_msg =
                                    LexiWarsServerMessage::Validate { msg: reason };
                                broadcast_to_player(player.id, &validation_msg, connections).await;
                                continue;
                            }
                        } else {
                            tracing::error!("fix invalid rule index {}", room.rule_index);
                        }

                        // check if word is valid
                        if !words.contains(&cleaned_word) {
                            let validation_msg = LexiWarsServerMessage::Validate {
                                msg: "Invalid word".to_string(),
                            };
                            broadcast_to_player(player.id, &validation_msg, connections).await;
                            tracing::info!(
                                "invalid word from {}: {}",
                                player.wallet_address,
                                cleaned_word
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

                            if let Some(current_player) =
                                room.players.iter().find(|p| p.id == next_id)
                            {
                                let next_turn_msg = LexiWarsServerMessage::Turn {
                                    current_turn: current_player.clone(),
                                };
                                broadcast_to_room(&next_turn_msg, &room, &connections).await;
                            }
                        } else {
                            tracing::error!("couldn't find next player");
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
                        broadcast_word_entry_from_player(
                            player,
                            &cleaned_word,
                            &room,
                            &connections,
                        )
                        .await;
                    }
                }
            }
        }
    }
}
