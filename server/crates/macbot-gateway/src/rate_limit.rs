//! Persisted sliding-window limits for private and coordinator model calls.
use macbot_store::{Store, StoreError};
use macbot_tools::ToolCancellation;
use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const PATH: &str = "data/limits/model-calls.json";
const CALLS: usize = 60;
const WINDOW_MS: u64 = 60_000;

pub(crate) struct ModelRateLimiter {
    store: Store,
    lock: Mutex<()>,
    limit: usize,
    window_ms: u64,
}

impl ModelRateLimiter {
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store,
            lock: Mutex::new(()),
            limit: CALLS,
            window_ms: WINDOW_MS,
        }
    }

    // The lock covers reading the fresh snapshot and persisting a reservation.
    // All per-run engines share this limiter through ExecutionState.
    async fn reserve(&self, bucket: &str, now: u64) -> Result<Option<u64>, StoreError> {
        let _guard = self.lock.lock().await;
        let mut calls = self
            .store
            .read_snapshot::<HashMap<String, Vec<u64>>>(PATH)?
            .unwrap_or_default();
        let timestamps = calls.entry(bucket.to_owned()).or_default();
        timestamps.retain(|at| now.saturating_sub(*at) < self.window_ms);
        timestamps.sort_unstable();
        if timestamps.len() >= self.limit {
            return Ok(Some(
                timestamps[0]
                    .saturating_add(self.window_ms)
                    .saturating_sub(now)
                    .max(1),
            ));
        }
        timestamps.push(now);
        self.store.write_snapshot(PATH, &calls)?;
        Ok(None)
    }

    pub(crate) async fn acquire(
        &self,
        bucket: &str,
        cancellation: &ToolCancellation,
    ) -> Result<bool, StoreError> {
        loop {
            if cancellation.is_cancelled() {
                return Ok(false);
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let Some(delay) = self.reserve(bucket, now).await? else {
                return Ok(!cancellation.is_cancelled());
            };
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay.min(self.window_ms))) => {},
                _ = cancellation.cancelled() => return Ok(false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::join_all;
    use tempfile::tempdir;

    #[tokio::test]
    async fn concurrent_reservations_are_bounded_and_survive_reopen() {
        let home = tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let limiter = ModelRateLimiter {
            store: store.clone(),
            lock: Mutex::new(()),
            limit: 2,
            window_ms: 60_000,
        };
        let results = join_all((0..16).map(|_| limiter.reserve("worker:private", 1_000))).await;
        assert_eq!(
            results
                .iter()
                .filter(|result| result.as_ref().unwrap().is_none())
                .count(),
            2
        );
        assert_eq!(
            limiter.reserve("main:coordinate", 1_000).await.unwrap(),
            None
        );
        drop(limiter);
        drop(store);
        let store = Store::open(home.path()).unwrap();
        let restored = ModelRateLimiter {
            store,
            lock: Mutex::new(()),
            limit: 2,
            window_ms: 60_000,
        };
        assert_eq!(
            restored.reserve("worker:private", 60_999).await.unwrap(),
            Some(1)
        );
        assert_eq!(
            restored.reserve("worker:private", 61_000).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn waiting_can_be_cancelled_without_reserving_another_call() {
        let home = tempdir().unwrap();
        let limiter = ModelRateLimiter {
            store: Store::open(home.path()).unwrap(),
            lock: Mutex::new(()),
            limit: 1,
            window_ms: 60_000,
        };
        let cancellation = ToolCancellation::default();
        assert!(limiter
            .acquire("worker:private", &cancellation)
            .await
            .unwrap());
        let cancel = cancellation.clone();
        let task = async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        };
        let (result, _) = tokio::join!(limiter.acquire("worker:private", &cancellation), task);
        assert!(!result.unwrap());
        let entries = limiter
            .store
            .read_snapshot::<HashMap<String, Vec<u64>>>(PATH)
            .unwrap()
            .unwrap();
        assert_eq!(entries["worker:private"].len(), 1);
    }
}
