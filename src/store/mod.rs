//! Local contest persistence. History and statistics describe the host's local
//! results, never fetched Codeforces history or synthetic opponents.
//!
//! Snapshots preserve problem metadata, ordered standings and bot schedules for
//! offline exports. Participant session tokens are deliberately never serialized.

pub mod db;

pub use db::Store;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub id: String,
    pub mode: String,
    pub started_at: i64,
    pub problem_count: usize,
    pub solved: u32,
    /// ICPC penalty in seconds.
    pub penalty: u64,
    pub host_handle: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TagStats {
    /// Assigned problems, including problems with no submissions.
    pub problems: usize,
    pub solved: usize,
    /// A fraction in the range 0..=1, or zero for an empty sample.
    pub solve_rate: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub total_contests: usize,
    pub total_problems: usize,
    pub total_solved: usize,
    pub solve_rate: f64,
    /// Mean host penalty per contest, in seconds.
    pub average_penalty: f64,
    /// Consecutive UTC dates with a saved contest, ending today or yesterday.
    pub current_streak: u32,
    pub per_tag: BTreeMap<String, TagStats>,
    /// Most recently fetched public-history analysis, distinct from local results.
    pub history_profile: Option<HistoryProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryProfile {
    pub handle: String,
    pub rating: u32,
    pub updated_at: i64,
    pub tags: BTreeMap<String, crate::domain::weakness::TagStats>,
    pub weights: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewEntry {
    pub problem_key: String,
    /// The contest in which this problem was first queued.
    pub contest_id: String,
    pub name: String,
    pub url: String,
    pub rating: Option<u32>,
    pub tags: Vec<String>,
    pub added_at: i64,
    pub due_at: i64,
    pub interval_days: u32,
    pub ease: f64,
    pub lapses: u32,
}
