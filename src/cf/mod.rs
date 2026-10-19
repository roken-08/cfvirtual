//! Read-only Codeforces API access and a persistent problemset cache.
//!
//! The problemset API does not include contest names. We exclude recognizable
//! nonstandard markers in problem names/tags, but cannot reliably identify every
//! April Fools, Kotlin Heroes, Q#, or Unknown Language contest from this API alone.
//! Unrated problems retain `rating: None`; contest selection must exclude them.

mod cache;
mod client;
mod error;
mod model;

pub use client::CfClient;
pub use error::CfError;
pub use model::UserInfo;
