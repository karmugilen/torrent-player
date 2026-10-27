//! 256 MiB SHA-1 throughput (acceptance).

use wt_core_hashing_standalone::hashing::{
    sha1_throughput_mib_s, selected_backend, throughput_mib_s, DEFAULT_HASHERS_PER_TORRENT,
    SoftSha1,
};

#[test]
fn sha1_256mib_pool_throughput() {
    const PAYLOAD: usize = 256 * 1024 * 1024;
    let workers = DEFAULT_HASHERS_PER_TORRENT.max(2);
    let _ = throughput_mib_s::<SoftSha1>(8 * 1024 * 1024, 1);
    let kind = selected_backend();
    let chosen = sha1_throughput_mib_s(PAYLOAD, workers);
    println!("chosen={kind:?}={chosen:.1} MiB/s workers={workers} payload=256 MiB");
    assert!(
        chosen > 50.0,
        "selected SHA-1 throughput too low: {chosen:.1} MiB/s ({kind:?})"
    );
}
