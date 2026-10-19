use std::{path::PathBuf, sync::Arc};

/// Failures from the public API, transport, or local problemset cache.
#[derive(Debug, thiserror::Error)]
pub enum CfError {
    #[error("could not initialize the Codeforces HTTP client: {0}")]
    ClientInitialization(#[source] Arc<reqwest::Error>),
    #[error("Codeforces {endpoint} timed out: {source}")]
    Timeout {
        endpoint: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Codeforces {endpoint} request failed: {source}")]
    Transport {
        endpoint: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Codeforces {endpoint} returned HTTP {status}")]
    Http {
        endpoint: &'static str,
        status: reqwest::StatusCode,
    },
    #[error("Codeforces {endpoint} failed: {comment}")]
    Api {
        endpoint: &'static str,
        comment: String,
    },
    #[error("invalid Codeforces handle: {comment}")]
    InvalidHandle { comment: String },
    #[error("could not decode Codeforces {endpoint} JSON: {source}")]
    Decode {
        endpoint: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("unexpected Codeforces {endpoint} response: {message}")]
    Protocol {
        endpoint: &'static str,
        message: &'static str,
    },
    #[error("problemset cache I/O failed at {path}: {source}")]
    CacheIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid problemset cache JSON at {path}: {source}")]
    CacheJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported problemset cache version at {path}")]
    CacheVersion { path: PathBuf },
    #[error("system clock is before the Unix epoch: {0}")]
    Clock(#[source] std::time::SystemTimeError),
}

impl CfError {
    pub fn is_invalid_handle(&self) -> bool {
        matches!(self, Self::InvalidHandle { .. })
    }

    pub(super) fn request(endpoint: &'static str, source: reqwest::Error) -> Self {
        if source.is_timeout() {
            Self::Timeout { endpoint, source }
        } else {
            Self::Transport { endpoint, source }
        }
    }

    pub(super) fn api(endpoint: &'static str, comment: String) -> Self {
        let lower = comment.to_ascii_lowercase();
        if lower.contains("handle")
            && (lower.contains("not found")
                || lower.contains("invalid")
                || lower.contains("should not be empty"))
        {
            Self::InvalidHandle { comment }
        } else {
            Self::Api { endpoint, comment }
        }
    }

    pub(super) fn is_rate_limited(&self) -> bool {
        match self {
            Self::Http { status, .. } => *status == reqwest::StatusCode::TOO_MANY_REQUESTS,
            Self::Api { comment, .. } => {
                comment.to_ascii_lowercase().contains("call limit exceeded")
            }
            _ => false,
        }
    }
}
