use super::CfError;
use crate::domain::Problem;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{fs, io::AsyncWriteExt};

const CACHE_VERSION: u32 = 1;
const MAX_AGE_SECONDS: u64 = 24 * 60 * 60;

#[derive(Deserialize)]
pub(super) struct Snapshot {
    version: u32,
    fetched_at: u64,
    pub problems: Vec<Problem>,
}

impl Snapshot {
    pub fn is_fresh(&self) -> bool {
        now()
            .ok()
            .and_then(|now| now.checked_sub(self.fetched_at))
            .is_some_and(|age| age < MAX_AGE_SECONDS)
    }
}

fn now() -> Result<u64, CfError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(CfError::Clock)
}

pub(super) async fn read(path: &Path) -> Result<Option<Snapshot>, CfError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CfError::CacheIo {
                path: path.to_owned(),
                source,
            });
        }
    };
    let snapshot: Snapshot =
        serde_json::from_slice(&bytes).map_err(|source| CfError::CacheJson {
            path: path.to_owned(),
            source,
        })?;
    if snapshot.version != CACHE_VERSION {
        return Err(CfError::CacheVersion {
            path: path.to_owned(),
        });
    }
    Ok(Some(snapshot))
}

pub(super) async fn write(path: &Path, problems: &[Problem]) -> Result<(), CfError> {
    #[derive(Serialize)]
    struct StoredSnapshot<'a> {
        version: u32,
        fetched_at: u64,
        problems: &'a [Problem],
    }
    let snapshot = StoredSnapshot {
        version: CACHE_VERSION,
        fetched_at: now()?,
        problems,
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|source| CfError::CacheJson {
        path: path.to_owned(),
        source,
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .await
        .map_err(|source| CfError::CacheIo {
            path: parent.to_owned(),
            source,
        })?;

    // A unique sibling keeps rename on the same filesystem and prevents readers
    // (including a new process) from ever observing a partially written cache.
    let temporary = parent.join(format!(
        ".cfvirtual-problemset-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(&temporary, path).await
    }
    .await;
    if let Err(source) = result {
        let _ = fs::remove_file(&temporary).await;
        return Err(CfError::CacheIo {
            path: path.to_owned(),
            source,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_freshness_rejects_expired_and_future_timestamps() {
        let current = now().expect("current Unix time");
        let mut snapshot = Snapshot {
            version: CACHE_VERSION,
            fetched_at: current,
            problems: Vec::new(),
        };
        assert!(snapshot.is_fresh());
        snapshot.fetched_at = current - MAX_AGE_SECONDS;
        assert!(!snapshot.is_fresh());
        snapshot.fetched_at = current + MAX_AGE_SECONDS;
        assert!(!snapshot.is_fresh());
    }

    #[tokio::test]
    async fn failed_atomic_replacement_preserves_target_and_cleans_temporary_file() {
        let directory = tempfile::tempdir().expect("cache directory");
        let target = directory.path().join("existing-directory");
        fs::create_dir(&target)
            .await
            .expect("create target directory");
        assert!(matches!(
            write(&target, &[]).await,
            Err(CfError::CacheIo { .. })
        ));
        assert!(fs::metadata(&target)
            .await
            .expect("target preserved")
            .is_dir());
        let mut entries = fs::read_dir(directory.path()).await.expect("cache entries");
        assert!(entries.next_entry().await.expect("target entry").is_some());
        assert!(entries
            .next_entry()
            .await
            .expect("temporary file cleaned")
            .is_none());
    }
}
