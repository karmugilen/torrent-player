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
        let lock = self.accounts.lock();
        if let Ok(bytes) = serde_json::to_vec(&*lock) {
            let tmp = self.path.with_extension("tmp");
            if fs::write(&tmp, bytes).is_ok() {
                let _ = fs::rename(tmp, &self.path);
            }
        }
    }
}
