use axum::extract::ws::{Message, WebSocket};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use futures::stream::SplitSink;
use mediasoup::{router::Router, worker::Worker};
use std::{collections::HashMap, sync::Arc};
use teloxide::Bot;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub rooms: SharedRooms,
    pub connections: ConnectionInfoMap, // For lobby and lexi-wars
    pub chat_connections: ChatConnectionInfoMap, // For chat only
    pub redis: RedisClient,
    pub lobby_join_requests: LobbyJoinRequests,
    pub bot: Bot,
    pub lobby_countdowns: LobbyCountdowns,
    pub chat_histories: ChatHistories,
    // New voice chat components
    pub mediasoup_worker: MediasoupWorker,
    pub voice_rooms: VoiceRooms,
    pub voice_participants: VoiceParticipants,
}

use crate::models::{
    chat::{ChatMessage, VoiceParticipant},
    game::GameRoom,
    lobby::JoinRequest,
};

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

// New voice chat structs
#[derive(Debug)]
pub struct VoiceRoom {
    pub room_id: Uuid,
    pub router: Router,
    pub participants: HashMap<Uuid, VoiceParticipant>,
}

impl VoiceRoom {
    pub fn new(room_id: Uuid, router: Router) -> Self {
        Self {
            room_id,
            router,
            participants: HashMap::new(),
        }
    }

    pub fn add_participant(&mut self, player_id: Uuid, participant: VoiceParticipant) {
        self.participants.insert(player_id, participant);
    }

    pub fn remove_participant(&mut self, player_id: &Uuid) -> Option<VoiceParticipant> {
        self.participants.remove(player_id)
    }

    pub fn get_participant(&self, player_id: &Uuid) -> Option<&VoiceParticipant> {
        self.participants.get(player_id)
    }

    pub fn get_participant_mut(&mut self, player_id: &Uuid) -> Option<&mut VoiceParticipant> {
        self.participants.get_mut(player_id)
    }

    pub fn get_all_participants(&self) -> Vec<VoiceParticipant> {
        self.participants.values().cloned().collect()
    }
}

pub type SharedRooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type ConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ConnectionInfo>>>>;

// Single chat connection per player, but track which room they're chatting in
pub type ChatConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ChatConnectionInfo>>>>;

pub type RedisClient = Pool<RedisConnectionManager>;

pub type LobbyJoinRequests = Arc<Mutex<HashMap<Uuid, Vec<JoinRequest>>>>;

pub type LobbyCountdowns = Arc<Mutex<HashMap<Uuid, CountdownState>>>;

pub type ChatHistories = Arc<Mutex<HashMap<Uuid, ChatHistory>>>; // TODO: move to ChatConnectionInfoMap

//
pub type MediasoupWorker = Arc<Mutex<Worker>>;

pub type VoiceRooms = Arc<Mutex<HashMap<Uuid, VoiceRoom>>>;

pub type VoiceParticipants = Arc<Mutex<HashMap<Uuid, HashMap<Uuid, VoiceParticipant>>>>; // room_id -> player_id -> participant
