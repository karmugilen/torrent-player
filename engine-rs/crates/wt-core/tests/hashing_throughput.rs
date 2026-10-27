//! SHA-1 pool throughput on a 256 MiB in-memory payload (acceptance).

use wt_core::hashing::{
    selected_backend, sha1_throughput_mib_s, throughput_mib_s, SoftSha1,
    DEFAULT_HASHERS_PER_TORRENT,
};

#[test]
fn sha1_256mib_pool_throughput() {
    const PAYLOAD: usize = 256 * 1024 * 1024;
    let workers = DEFAULT_HASHERS_PER_TORRENT.max(2);

    // Warmup (page in) + force backend selection microbench.
    let _ = throughput_mib_s::<SoftSha1>(8 * 1024 * 1024, 1);
    let kind = selected_backend();

    let chosen = sha1_throughput_mib_s(PAYLOAD, workers);
    println!("chosen={kind:?}={chosen:.1} MiB/s workers={workers} payload=256 MiB");
    assert!(
        chosen > 50.0,
        "selected SHA-1 throughput too low: {chosen:.1} MiB/s ({kind:?})"
    );
    #[cfg(target_arch = "aarch64")]
    assert_eq!(kind, wt_core::hashing::Sha1BackendKind::Ring);
}
