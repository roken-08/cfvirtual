use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProblemKey {
    pub contest_id: u32,
    pub index: String,
}

impl std::fmt::Display for ProblemKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.contest_id, self.index)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Problem {
    pub key: ProblemKey,
    pub name: String,
    pub rating: Option<u32>,
    pub tags: Vec<String>,
    pub solved_count: Option<u32>,
}

impl Problem {
    pub fn url(&self) -> String {
        format!(
            "https://codeforces.com/problemset/problem/{}/{}",
            self.key.contest_id, self.key.index
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    Weak,
    Core,
    Stretch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContestProblem {
    pub slot: char,
    pub problem: Problem,
    pub tier: Tier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub handle: String,
    pub is_host: bool,
    pub is_spectator: bool,
    #[serde(default, skip_serializing)]
    pub token: String,
    #[serde(default = "connected_default")]
    pub connected: bool,
}
fn connected_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSolve {
    pub slot: char,
    pub accepted_at_sec: u32,
    pub rejected_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bot {
    pub name: String,
    pub rating: u32,
    pub solves: Vec<BotSolve>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contest {
    pub id: String,
    pub code: Option<String>,
    pub host_handle: String,
    pub host_rating: u32,
    pub started_at: i64,
    pub duration_sec: u32,
    pub problems: Vec<ContestProblem>,
    pub participants: Vec<Participant>,
    pub bots: Vec<Bot>,
    pub seed: u64,
    pub mode: String,
    pub blind: bool,
    pub ended_at: Option<i64>,
}

impl Contest {
    pub fn deadline(&self) -> i64 {
        self.ended_at
            .unwrap_or(self.started_at + i64::from(self.duration_sec))
            .min(self.started_at + i64::from(self.duration_sec))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    pub id: u64,
    pub problem: ProblemKey,
    pub created_at: i64,
    pub verdict: Option<String>,
    pub handles: Vec<String>,
    pub tags: Vec<String>,
    pub rating: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemStatus {
    pub solved: bool,
    pub accepted_at_sec: Option<u32>,
    pub rejected: u32,
    pub attempts: u32,
    pub first_attempt_at_sec: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub name: String,
    pub is_bot: bool,
    pub rating: u32,
    pub solved: u32,
    pub penalty: u64,
    pub last_ac: u32,
    pub problems: BTreeMap<char, ProblemStatus>,
}
