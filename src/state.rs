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
    pub redis: RedisClient,
    pub connections: ConnectionInfoMap,
    pub chat_connections: ChatConnectionInfoMap,
    pub chat_histories: ChatHistories,
    pub rooms: SharedRooms,
    pub lobby_join_requests: LobbyJoinRequests,
    pub lobby_countdowns: LobbyCountdowns,
    pub telegram_bot: Bot,
    pub voice_connections: crate::ws::handlers::chat::voice::VoiceConnections,
    pub worker_manager: Arc<mediasoup::worker_manager::WorkerManager>,
}

use crate::models::{chat::ChatMessage, game::GameRoom, lobby::JoinRequest};

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
    pub is_running: bool,
}

impl Default for CountdownState {
    fn default() -> Self {
        Self {
            current_time: 15,
            is_running: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatHistory {
    messages: Vec<ChatMessage>,
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

pub type SharedRooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type ConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ConnectionInfo>>>>;

// Single chat connection per player, but track which room they're chatting in
pub type ChatConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ChatConnectionInfo>>>>;

pub type ChatHistories = Arc<Mutex<HashMap<Uuid, ChatHistory>>>;

pub type LobbyJoinRequests = Arc<Mutex<HashMap<Uuid, Vec<JoinRequest>>>>;

pub type LobbyCountdowns = Arc<Mutex<HashMap<Uuid, CountdownState>>>;

pub type RedisClient = Pool<RedisConnectionManager>;
