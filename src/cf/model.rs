use crate::domain::{Problem, ProblemKey, Submission};
use serde::Deserialize;
use std::collections::HashMap;

/// A validated handle and its current rating (1200 for an unrated user).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub handle: String,
    pub rating: u32,
}

#[derive(Deserialize)]
pub(super) struct ApiResponse<T> {
    pub status: String,
    pub comment: Option<String>,
    pub result: Option<T>,
}

#[derive(Deserialize)]
pub(super) struct ApiUser {
    pub handle: String,
    pub rating: Option<u32>,
}

impl From<ApiUser> for UserInfo {
    fn from(user: ApiUser) -> Self {
        Self {
            handle: user.handle,
            rating: user.rating.unwrap_or(1200),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiSubmission {
    id: u64,
    contest_id: Option<u32>,
    creation_time_seconds: i64,
    problem: ApiProblem,
    verdict: Option<String>,
    author: ApiParty,
}

#[derive(Deserialize)]
struct ApiParty {
    members: Vec<ApiMember>,
}

#[derive(Deserialize)]
struct ApiMember {
    handle: String,
}

impl ApiSubmission {
    pub fn into_submission(self) -> Option<Submission> {
        // The submission's contestId is authoritative. Do not infer one from a
        // non-contest submission's embedded problem or problemsetName.
        Some(Submission {
            id: self.id,
            problem: ProblemKey {
                contest_id: self.contest_id?,
                index: self.problem.index,
            },
            created_at: self.creation_time_seconds,
            verdict: self.verdict,
            handles: self.author.members.into_iter().map(|m| m.handle).collect(),
            tags: self.problem.tags,
            rating: self.problem.rating,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiProblem {
    contest_id: Option<u32>,
    index: String,
    name: String,
    rating: Option<u32>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiProblemset {
    problems: Vec<ApiProblem>,
    problem_statistics: Vec<ApiProblemStatistics>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiProblemStatistics {
    contest_id: Option<u32>,
    index: String,
    solved_count: u32,
}

impl ApiProblemset {
    pub fn into_problems(self) -> Vec<Problem> {
        let counts: HashMap<_, _> = self
            .problem_statistics
            .into_iter()
            .filter_map(|stat| {
                Some((
                    ProblemKey {
                        contest_id: stat.contest_id?,
                        index: stat.index,
                    },
                    stat.solved_count,
                ))
            })
            .collect();
        self.problems
            .into_iter()
            .filter(|p| !nonstandard(&p.name) && !p.tags.iter().any(|tag| nonstandard(tag)))
            .filter_map(|p| {
                let key = ProblemKey {
                    contest_id: p.contest_id?,
                    index: p.index,
                };
                let solved_count = counts.get(&key).copied();
                Some(Problem {
                    key,
                    name: p.name,
                    rating: p.rating,
                    tags: p.tags,
                    solved_count,
                })
            })
            .collect()
    }
}

fn nonstandard(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace(['-', '_'], " ");
    [
        "april fools",
        "april fool",
        "kotlin heroes",
        "q#",
        "unknown language",
        "*special",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}
