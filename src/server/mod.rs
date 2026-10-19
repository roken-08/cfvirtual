//! Authoritative rooms. Bind TCP in the caller and serve [`router`].
mod poller;
pub mod proto;
mod room;
mod ws;

#[cfg(test)]
mod tests;

pub use room::{CreatedRoom, RoomError, State};

use crate::{cf::CfClient, domain::Submission, service::ContestOptions};
use anyhow::Result;
use axum::{routing::get, Router};
use std::{future::Future, path::PathBuf, pin::Pin};

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct ValidatedHandle {
    pub handle: String,
    pub rating: u32,
}

/// Trusted server-side adapter for handle validation, preparation and polling.
///
/// Production callers should use `State::new`. `State::with_backend` supports
/// deterministic offline tests or a trusted alternate data source, without a
/// network-accessible "disable validation" switch. Implementations must validate
/// handles, return only public submissions, and apply their own API rate limit.
pub trait RoomBackend: Send + Sync + 'static {
    fn validate_handle<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, ValidatedHandle>;
    fn recent<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, Vec<Submission>>;
    fn prepare<'a>(
        &'a self,
        options: &'a ContestOptions,
    ) -> BackendFuture<'a, crate::domain::Contest>;
}

struct CodeforcesBackend {
    api: CfClient,
    data_dir: PathBuf,
}

impl RoomBackend for CodeforcesBackend {
    fn validate_handle<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, ValidatedHandle> {
        Box::pin(async move {
            let user = self.api.user_info(handle).await?;
            Ok(ValidatedHandle {
                handle: user.handle,
                rating: user.rating,
            })
        })
    }
    fn recent<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, Vec<Submission>> {
        Box::pin(async move { Ok(self.api.recent(handle).await?) })
    }
    fn prepare<'a>(
        &'a self,
        options: &'a ContestOptions,
    ) -> BackendFuture<'a, crate::domain::Contest> {
        Box::pin(async move {
            crate::service::prepare(options, &self.api, &self.data_dir, "room").await
        })
    }
}

/// WebSockets are accepted at both `/` and `/ws` (ws://host:port works).
pub fn router(state: State) -> Router {
    Router::new()
        .route("/", get(ws::upgrade))
        .route("/ws", get(ws::upgrade))
        .with_state(state)
}
