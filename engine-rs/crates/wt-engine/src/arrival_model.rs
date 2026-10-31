//! Go-compatible learned model persistence. Disk I/O runs outside peer and
//! telemetry tasks; Android process death is covered by a one-minute save.

use std::sync::{atomic::Ordering, Arc};

use wt_policy::arrival_persist::ArrivalModelPersist;

use super::EngineServer;
use crate::types::ArrivalStats;

impl EngineServer {
    pub(super) fn arrival_stats(&self) -> ArrivalStats {
        let stats = self.client.arrival_stats();
        ArrivalStats {
            samples: stats.samples.min(i64::MAX as u64) as i64,
            network_err_ms: stats.mean_abs_error_ms,
            using: self.learned_arrival.load(Ordering::SeqCst),
        }
    }

    pub(super) fn load_arrival_model(&self) {
        let path = ArrivalModelPersist::new(&self.download_dir).path();
        if let Ok(state) = std::fs::read(path) {
            // Ignore older shapes or invalid files and learn from scratch.
            if self.client.set_arrival_state(&state).is_ok() {
                self.arrival_saved_samples
                    .store(self.client.arrival_stats().samples, Ordering::Relaxed);
            }
        }
    }

    pub(super) fn save_arrival_model(&self) {
        let _saving = self.arrival_save_lock.lock();
        let samples = self.client.arrival_stats().samples;
        if samples == 0 || samples == self.arrival_saved_samples.load(Ordering::Relaxed) {
            return;
        }
        let Ok(state) = self.client.arrival_state() else {
            return;
        };
        if state.is_empty() {
            return;
        }
        let persist = ArrivalModelPersist::new(&self.download_dir);
        let path = persist.path();
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let tmp = persist.tmp_path();
        if std::fs::write(&tmp, state).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.arrival_saved_samples.store(samples, Ordering::Relaxed);
        }
    }

    pub(super) fn start_arrival_saver(server: &Arc<Self>) {
        let weak = Arc::downgrade(server);
        server.client.inner.tokio_handle.spawn(async move {
            loop {
                tokio::time::sleep(ArrivalModelPersist::save_every()).await;
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if server.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                let handle = server.client.inner.tokio_handle.clone();
                let _ = handle
                    .spawn_blocking(move || server.save_arrival_model())
                    .await;
            }
        });
    }
}
