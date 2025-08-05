use axum::extract::ws::{Message, WebSocket};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use futures::stream::SplitSink;
use std::{collections::HashMap, sync::Arc};
use teloxide::Bot;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub lexi_wars_lobbies: LexiWarsLobbies,
    pub connections: ConnectionInfoMap,
    pub chat_connections: ChatConnectionInfoMap,
    pub redis: RedisClient,
    pub lobby_join_requests: LobbyJoinRequests,
    pub bot: Bot,
    pub lobby_countdowns: LobbyCountdowns,
    pub chat_histories: ChatHistories,
}

use crate::models::{chat::ChatMessage, game::LexiWars, lobby::JoinRequest};

#[derive(Debug)]
pub struct ConnectionInfo {
    pub sender: Arc<Mutex<SplitSink<WebSocket, Message>>>,
}

#[derive(Debug)]
pub struct ChatConnectionInfo {
    pub sender: Arc<Mutex<SplitSink<WebSocket, Message>>>,
}

#[derive(Debug, Clone)]
pub struct CountdownState {
    pub current_time: u32,
}

impl Default for CountdownState {
    fn default() -> Self {
        Self { current_time: 15 }
    }
}

#[derive(Debug, Clone)]
pub struct ChatHistory {
    pub messages: Vec<ChatMessage>,
}

impl ChatHistory {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);

        // Keep only the last 50 messages
        if self.messages.len() > 50 {
            self.messages.remove(0);
        }
    }

    pub fn get_messages(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }
}

pub type LexiWarsLobbies = Arc<Mutex<HashMap<Uuid, LexiWars>>>;

pub type ConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ConnectionInfo>>>>;

// Single chat connection per player, but track which room they're chatting in
pub type ChatConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ChatConnectionInfo>>>>;

pub type RedisClient = Pool<RedisConnectionManager>;

pub type LobbyJoinRequests = Arc<Mutex<HashMap<Uuid, Vec<JoinRequest>>>>;

pub type LobbyCountdowns = Arc<Mutex<HashMap<Uuid, CountdownState>>>;

pub type ChatHistories = Arc<Mutex<HashMap<Uuid, ChatHistory>>>; // TODO: move to ChatConnectionInfoMap
