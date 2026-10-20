//! Version-one JSON wire types. Only the envelopes contain the protocol version.
use crate::{domain::ProblemStatus, service::ContestOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: u8 = 1;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const OUTBOUND_CAPACITY: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
pub struct ClientEnvelope {
    pub v: u8,
    #[serde(flatten)]
    pub message: ClientMessage,
}

impl From<ClientMessage> for ClientEnvelope {
    fn from(message: ClientMessage) -> Self {
        Self {
            v: VERSION,
            message,
        }
    }
}

// Deliberately no Debug: requests can contain passwords and bearer tokens.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        code: String,
        handle: String,
        #[serde(default)]
        password: Option<String>,
    },
    Reconnect {
        code: String,
        token: String,
    },
    Spectate {
        code: String,
        #[serde(default)]
        password: Option<String>,
    },
    Create {
        options: ContestOptions,
        #[serde(default)]
        password: Option<String>,
    },
    Start,
    End,
    Ping {
        t: i64,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerEnvelope {
    pub v: u8,
    #[serde(flatten)]
    pub message: ServerMessage,
}

impl From<ServerMessage> for ServerEnvelope {
    fn from(message: ServerMessage) -> Self {
        Self {
            v: VERSION,
            message,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        participant_id: String,
        token: String,
        state: Snapshot,
    },
    Roster {
        participants: Vec<PublicParticipant>,
        bots: Vec<PublicBot>,
        settings: Settings,
    },
    ContestStarted {
        contest_id: String,
        started_at: i64,
        duration_sec: u32,
        problems: Vec<PublicProblem>,
        blind: bool,
        server_time: i64,
    },
    Tick {
        server_time: i64,
        standings: Vec<PublicStanding>,
        problem_status: OwnProblemStatus,
        state: Snapshot,
    },
    ContestEnded {
        reason: EndReason,
        standings: Vec<PublicStanding>,
        state: Snapshot,
    },
    Error {
        code: String,
        message: String,
    },
    Pong {
        t: i64,
        server_time: i64,
    },
    #[serde(other)]
    Unknown,
}

pub type OwnProblemStatus = BTreeMap<String, BTreeMap<char, ProblemStatus>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Lobby,
    Countdown,
    Running,
    Finishing,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Timeout,
    Host,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicParticipant {
    pub participant_id: String,
    pub handle: String,
    pub is_host: bool,
    pub is_spectator: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicBot {
    pub name: String,
    pub rating: u32,
    pub is_bot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicProblem {
    pub slot: char,
    pub contest_id: u32,
    pub index: String,
    pub name: String,
    pub url: String,
    pub rating: Option<u32>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub duration_sec: u32,
    pub problem_count: usize,
    pub blind: bool,
    pub password_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicStanding {
    pub name: String,
    pub is_bot: bool,
    pub rating: u32,
    pub solved: u32,
    pub penalty: u64,
    pub last_ac: u32,
}

/// Full reconnect/UI state. No domain Contest/Standing is serialized to the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub code: String,
    pub contest_id: String,
    pub host_handle: String,
    pub phase: Phase,
    pub started_at: i64,
    pub deadline: Option<i64>,
    pub ended_at: Option<i64>,
    pub end_reason: Option<EndReason>,
    pub server_time: i64,
    pub settings: Settings,
    pub participants: Vec<PublicParticipant>,
    pub bots: Vec<PublicBot>,
    pub problems: Vec<PublicProblem>,
    pub standings: Vec<PublicStanding>,
    pub problem_status: OwnProblemStatus,
}
