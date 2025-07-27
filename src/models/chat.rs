use crate::models::game::Player;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChatClientMessage {
    Chat { text: String },
    Ping { ts: u64 },
    Mic { enabled: bool },
    Mute { muted: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Uuid,
    pub text: String,
    pub sender: Player,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceParticipant {
    pub player: Player,
    pub mic_enabled: bool,
    pub is_muted: bool,
    pub is_speaking: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChatServerMessage {
    PermitChat { allowed: bool },
    Chat { message: ChatMessage },
    ChatHistory { messages: Vec<ChatMessage> },
    Pong { ts: u64, pong: u64 },
    Error { message: String },
    // Voice chat messages
    VoicePermit { allowed: bool },
    VoiceParticipants { participants: Vec<VoiceParticipant> },
    VoiceParticipantUpdate { participant: VoiceParticipant },
}

impl ChatServerMessage {
    pub fn should_queue(&self) -> bool {
        match self {
            // Time-sensitive messages that should NOT be queued
            ChatServerMessage::Pong { .. } => false,

            // Important messages that SHOULD be queued
            ChatServerMessage::PermitChat { .. } => true,
            ChatServerMessage::Chat { .. } => true,
            ChatServerMessage::ChatHistory { .. } => true,
            ChatServerMessage::Error { .. } => true,
            ChatServerMessage::VoicePermit { .. } => true,
            ChatServerMessage::VoiceParticipants { .. } => true,
            ChatServerMessage::VoiceParticipantUpdate { .. } => true,
        }
    }
}
