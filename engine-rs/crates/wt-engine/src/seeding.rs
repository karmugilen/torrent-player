//! Seeding account tracking and policy enforcement.
//!
//! Port of `engine-go/seeding.go`.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeedAccount {
    pub uploaded: i64,
    pub downloaded: i64,
    pub seconds: i64,
}

pub struct SeedingManager {
    accounts: Mutex<HashMap<String, SeedAccount>>,
    save_lock: Mutex<()>,
    path: PathBuf,
}

impl SeedingManager {
    pub fn new(download_dir: impl AsRef<Path>) -> Self {
        let path = download_dir.as_ref().join(".seed-accounts.json");
        let mut map = HashMap::new();
        if let Ok(data) = fs::read(&path) {
            if let Ok(loaded) = serde_json::from_slice::<HashMap<String, SeedAccount>>(&data) {
                map = loaded;
            }
        }
        Self {
            accounts: Mutex::new(map),
            save_lock: Mutex::new(()),
            path,
        }
    }

    pub fn get_account(&self, hash: &str) -> SeedAccount {
        self.accounts.lock().get(hash).copied().unwrap_or_default()
    }

    pub fn update_account(&self, hash: &str, account: SeedAccount) {
        self.accounts.lock().insert(hash.to_string(), account);
    }

    pub fn save(&self) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _serial = self.save_lock.lock();
        let bytes = serde_json::to_vec(&*self.accounts.lock());
        if let Ok(bytes) = bytes {
            let tmp = self.path.with_extension("tmp");
            let result = (|| -> std::io::Result<()> {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .mode(0o600)
                    .open(&tmp)?;
                file.write_all(&bytes)?;
                fs::rename(&tmp, &self.path)
            })();
            if result.is_err() {
                let _ = fs::remove_file(tmp);
            }
        }
    }
}

/// Lifetime totals include the previous engine session's persisted account.
pub fn seed_totals(rec: &crate::server::TorrentRecord) -> SeedAccount {
    let stats = rec.torrent.stats().conn_stats;
    SeedAccount {
        uploaded: rec.seed_base.uploaded + stats.bytes_written_data,
        downloaded: rec.seed_base.downloaded + stats.bytes_read_data,
        seconds: rec.seed_base.seconds,
    }
}

impl crate::server::EngineServer {
    pub(crate) fn apply_seeding_policy(&self, rec: &mut crate::server::TorrentRecord, tick: bool) {
        use std::sync::atomic::Ordering;
        let was = rec.seeding;
        rec.seeding = false;
        let active = self.payload_transfers_allowed.load(Ordering::SeqCst)
            && rec.configured
            && !rec.paused
            && !rec.checking
            && !rec.torrent.closed()
            && rec.error.is_none();
        if !active || rec.direct_play || rec.torrent.info().is_none() || rec.selected.is_empty() {
            if was {
                self.notify_change();
            }
            return;
        }
        let files = rec.torrent.files();
        let selected: Vec<_> = rec.selected.iter().filter_map(|i| files.get(*i)).collect();
        let size: i64 = selected.iter().map(|f| f.length()).sum();
        let complete = selected.len() == rec.selected.len()
            && size > 0
            && selected.iter().all(|f| f.bytes_completed() >= f.length());
        if !complete {
            rec.torrent.allow_data_upload();
        } else {
            let account = seed_totals(rec);
            let ratio = account.uploaded as f64 / size.max(account.downloaded).max(1) as f64;
            let allowed = self.seeding_enabled.load(Ordering::SeqCst)
                && self.seeding_allowed.load(Ordering::SeqCst)
                && ratio < self.seed_ratio_milli.load(Ordering::SeqCst) as f64 / 1000.0
                && account.seconds
                    < self
                        .seed_time_minutes
                        .load(Ordering::SeqCst)
                        .saturating_mul(60);
            if allowed {
                rec.torrent.allow_data_upload();
                rec.seeding = true;
                if tick {
                    rec.seed_base.seconds += 1;
                }
            } else {
                rec.torrent.disallow_data_upload();
            }
        }
        if was != rec.seeding {
            self.notify_change();
        }
    }

    pub fn seed_tick(&self) {
        let records: Vec<_> = self.records.read().values().cloned().collect();
        for record in records {
            let mut rec = record.write();
            self.apply_seeding_policy(&mut rec, true);
            self.seeding
                .update_account(&rec.info_hash, seed_totals(&rec));
        }
    }
}
