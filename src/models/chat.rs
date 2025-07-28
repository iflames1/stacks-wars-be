use crate::models::game::Player;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChatClientMessage {
    Chat {
        text: String,
    },
    Ping {
        ts: u64,
    },
    // Voice chat client messages
    VoiceInit {
        rtp_capabilities: mediasoup::rtp_parameters::RtpCapabilities,
    },
    ConnectProducerTransport {
        dtls_parameters: mediasoup::data_structures::DtlsParameters,
    },
    ConnectConsumerTransport {
        dtls_parameters: mediasoup::data_structures::DtlsParameters,
    },
    Produce {
        kind: mediasoup::rtp_parameters::MediaKind,
        rtp_parameters: mediasoup::rtp_parameters::RtpParameters,
    },
    Consume {
        producer_id: mediasoup::producer::ProducerId,
    },
    ConsumerResume {
        id: mediasoup::consumer::ConsumerId,
    },
    Mic {
        enabled: bool,
    },
    Mute {
        muted: bool,
    },
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
#[serde(rename_all = "camelCase")]
pub struct TransportOptions {
    pub id: mediasoup::transport::TransportId,
    pub dtls_parameters: mediasoup::data_structures::DtlsParameters,
    pub ice_candidates: Vec<mediasoup::data_structures::IceCandidate>,
    pub ice_parameters: mediasoup::data_structures::IceParameters,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChatServerMessage {
    PermitChat {
        allowed: bool,
    },
    Chat {
        message: ChatMessage,
    },
    ChatHistory {
        messages: Vec<ChatMessage>,
    },
    Pong {
        ts: u64,
        pong: u64,
    },
    Error {
        message: String,
    },
    // Voice chat server messages
    VoiceInit {
        consumer_transport_options: TransportOptions,
        producer_transport_options: TransportOptions,
        router_rtp_capabilities: mediasoup::rtp_parameters::RtpCapabilitiesFinalized,
    },
    ConnectedProducerTransport,
    Produced {
        id: mediasoup::producer::ProducerId,
    },
    ConnectedConsumerTransport,
    Consumed {
        id: mediasoup::consumer::ConsumerId,
        producer_id: mediasoup::producer::ProducerId,
        kind: mediasoup::rtp_parameters::MediaKind,
        rtp_parameters: mediasoup::rtp_parameters::RtpParameters,
    },
    VoiceParticipants {
        participants: Vec<VoiceParticipant>,
    },
    VoiceParticipantUpdate {
        participant: VoiceParticipant,
    },
}

impl ChatServerMessage {
    pub fn should_queue(&self) -> bool {
        match self {
            // Time-sensitive messages that should NOT be queued
            ChatServerMessage::Pong { .. } => false,
            ChatServerMessage::ConnectedProducerTransport => false,
            ChatServerMessage::ConnectedConsumerTransport => false,

            // Important messages that SHOULD be queued
            ChatServerMessage::PermitChat { .. } => true,
            ChatServerMessage::Chat { .. } => true,
            ChatServerMessage::ChatHistory { .. } => true,
            ChatServerMessage::Error { .. } => true,
            ChatServerMessage::VoiceInit { .. } => true,
            ChatServerMessage::Produced { .. } => true,
            ChatServerMessage::Consumed { .. } => true,
            ChatServerMessage::VoiceParticipants { .. } => true,
            ChatServerMessage::VoiceParticipantUpdate { .. } => true,
        }
    }
}
