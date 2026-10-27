//! Engine-side arrival-model persistence (`engine-go/arrival_model.go`).
//!
//! The neural arrival model itself lives in `wt-core`. This module only decides
//! *where* and *when* the engine should save/load the opaque JSON blob so a
//! restart does not wipe learned weights. The engine performs the actual I/O.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::constants::ARRIVAL_SAVE_EVERY;

/// Opaque arrival-model stats the engine reads from wt-core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArrivalStats {
    pub samples: i64,
    pub network_err_ms: i64,
    pub using: bool,
}

/// Path and save-cadence helpers for the arrival model blob.
#[derive(Clone, Debug)]
pub struct ArrivalModelPersist {
    download_dir: PathBuf,
}

impl ArrivalModelPersist {
    pub fn new(download_dir: impl Into<PathBuf>) -> Self {
        Self {
            download_dir: download_dir.into(),
        }
    }

    /// `downloadDir/.cache/arrival-model.json`
    pub fn path(&self) -> PathBuf {
        self.download_dir.join(".cache").join("arrival-model.json")
    }

    pub fn tmp_path(&self) -> PathBuf {
        let mut p = self.path();
        let mut name = p.file_name().map(|s| s.to_os_string()).unwrap_or_default();
        name.push(".tmp");
        p.set_file_name(name);
        p
    }

    pub fn save_every() -> Duration {
        ARRIVAL_SAVE_EVERY
    }

    /// Save when the model learned more samples since the last successful save.
    pub fn should_save(last_saved_samples: i64, current: &ArrivalStats) -> bool {
        current.samples != last_saved_samples && current.samples > 0
    }

    /// Parent directory that must exist before writing.
    pub fn cache_dir(path: &Path) -> Option<&Path> {
        path.parent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_and_save_gate() {
        let p = ArrivalModelPersist::new("/data/webtor");
        assert_eq!(
            p.path(),
            PathBuf::from("/data/webtor/.cache/arrival-model.json")
        );
        assert!(!ArrivalModelPersist::should_save(
            0,
            &ArrivalStats {
                samples: 0,
                ..Default::default()
            }
        ));
        assert!(ArrivalModelPersist::should_save(
            0,
            &ArrivalStats {
                samples: 10,
                ..Default::default()
            }
        ));
        assert!(!ArrivalModelPersist::should_save(
            10,
            &ArrivalStats {
                samples: 10,
                ..Default::default()
            }
        ));
    }
}
