//! Pure Rust storage benchmarks comparing against Go engine storage.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wt_storage::{
    aggregate_piece_buckets, read_piece_checkpoint, write_piece_checkpoint, DocumentTorrentStorage,
    FileIdentity, PieceCheckpoint, PieceRange, PieceRun, TorrentStorage,
};
use wt_types::{FileInfo, Info, InfoHash};

// Allocation tracking
struct TrackingAlloc;
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for TrackingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL_ALLOC: TrackingAlloc = TrackingAlloc;

fn get_peak_rss_mib() -> f64 {
    unsafe {
        let mut r = std::mem::zeroed::<libc::rusage>();
        if libc::getrusage(libc::RUSAGE_SELF, &mut r) == 0 {
            // Linux ru_maxrss is in KiB
            r.ru_maxrss as f64 / 1024.0
        } else {
            0.0
        }
    }
}

const BENCH_TORRENT_BYTES: i64 = 256 * 1024 * 1024; // 256 MiB
const BENCH_PIECE_LENGTH: i64 = 1024 * 1024; // 1 MiB
const BENCH_NUM_PIECES: usize = 256;
const BENCH_CHUNK_SIZE: usize = 16 * 1024; // 16 KiB
const BENCH_CHUNKS_PER_PIECE: usize = (BENCH_PIECE_LENGTH as usize) / BENCH_CHUNK_SIZE;

fn create_bench_info() -> Info {
    Info {
        name: "bench-torrent.bin".into(),
        piece_length: BENCH_PIECE_LENGTH,
        pieces: vec![0u8; BENCH_NUM_PIECES * 20],
        files: vec![FileInfo {
            length: BENCH_TORRENT_BYTES,
            path: vec![],
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    }
}

// 1. Download Write-Back Storage
fn run_rust_download_write_seq(base_dir: &Path) -> Duration {
    let info = create_bench_info();
    let hash = InfoHash([0u8; 20]);
    let st = DocumentTorrentStorage::new(Some(info), hash, base_dir);

    let dest_path = base_dir.join("download-dest-seq.bin");
    let dest = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&dest_path)
        .expect("open dest");

    let fd = dest.as_raw_fd();
    st.attach_descriptors(&[Some(fd)], &[0]).expect("attach");

    let chunk = vec![0xAAu8; BENCH_CHUNK_SIZE];

    let start = Instant::now();
    for p in 0..BENCH_NUM_PIECES {
        let piece = st.piece_at(p);
        for c in 0..BENCH_CHUNKS_PER_PIECE {
            piece
                .write_at(&chunk, (c * BENCH_CHUNK_SIZE) as i64)
                .expect("write chunk");
        }
        piece.mark_complete().expect("mark complete");
    }
    // Close to flush all write-back data
    TorrentStorage::close(&*st).expect("close");
    let elapsed = start.elapsed();
    let _ = fs::remove_file(dest_path);
    elapsed
}

fn run_rust_download_read_seq(base_dir: &Path) -> Duration {
    let info = create_bench_info();
    let hash = InfoHash([0u8; 20]);
    let st = DocumentTorrentStorage::new(Some(info), hash, base_dir);

    let dest_path = base_dir.join("download-dest-readseq.bin");
    let dest = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&dest_path)
        .expect("open dest");

    let mut writer = &dest;
    let data = vec![0xAA; 1024 * 1024];
    for _ in 0..256 {
        writer.write_all(&data).expect("prefill");
    }
    let fd = dest.as_raw_fd();
    st.attach_descriptors(&[Some(fd)], &[0]).expect("attach");

    let mut buf = vec![0u8; BENCH_CHUNK_SIZE];
    let start = Instant::now();
    for p in 0..BENCH_NUM_PIECES {
        let piece = st.piece_at(p);
        for c in 0..BENCH_CHUNKS_PER_PIECE {
            piece
                .read_at(&mut buf, (c * BENCH_CHUNK_SIZE) as i64)
                .expect("read chunk");
        }
    }
    let elapsed = start.elapsed();
    let _ = TorrentStorage::close(&*st);
    let _ = fs::remove_file(dest_path);
    elapsed
}

fn make_permutation(n: usize, seed: u64) -> Vec<usize> {
    let mut perm: Vec<usize> = (0..n).collect();
    let mut state = seed;
    for i in (1..n).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state >> 33) as usize % (i + 1);
        perm.swap(i, j);
    }
    perm
}

fn run_rust_download_write_random(base_dir: &Path) -> Duration {
    let info = create_bench_info();
    let hash = InfoHash([0u8; 20]);
    let st = DocumentTorrentStorage::new(Some(info), hash, base_dir);

    let dest_path = base_dir.join("download-dest-rand.bin");
    let dest = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&dest_path)
        .expect("open dest");

    let fd = dest.as_raw_fd();
    st.attach_descriptors(&[Some(fd)], &[0]).expect("attach");

    let chunk = vec![0xBBu8; BENCH_CHUNK_SIZE];
    let perm = make_permutation(BENCH_NUM_PIECES, 42);

    let start = Instant::now();
    for &p in &perm {
        let piece = st.piece_at(p);
        for c in 0..BENCH_CHUNKS_PER_PIECE {
            piece
                .write_at(&chunk, (c * BENCH_CHUNK_SIZE) as i64)
                .expect("write chunk");
        }
        piece.mark_complete().expect("mark complete");
    }
    TorrentStorage::close(&*st).expect("close");
    let elapsed = start.elapsed();
    let _ = fs::remove_file(dest_path);
    elapsed
}

fn run_rust_download_read_random(base_dir: &Path) -> Duration {
    let info = create_bench_info();
    let hash = InfoHash([0u8; 20]);
    let st = DocumentTorrentStorage::new(Some(info), hash, base_dir);

    let dest_path = base_dir.join("download-dest-readrand.bin");
    let dest = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&dest_path)
        .expect("open dest");

    let mut writer = &dest;
    let data = vec![0xAA; 1024 * 1024];
    for _ in 0..256 {
        writer.write_all(&data).expect("prefill");
    }
    let fd = dest.as_raw_fd();
    st.attach_descriptors(&[Some(fd)], &[0]).expect("attach");

    let perm = make_permutation(BENCH_NUM_PIECES, 42);
    let mut buf = vec![0u8; BENCH_CHUNK_SIZE];

    let start = Instant::now();
    for &p in &perm {
        let piece = st.piece_at(p);
        for c in 0..BENCH_CHUNKS_PER_PIECE {
            piece
                .read_at(&mut buf, (c * BENCH_CHUNK_SIZE) as i64)
                .expect("read chunk");
        }
    }
    let elapsed = start.elapsed();
    let _ = TorrentStorage::close(&*st);
    let _ = fs::remove_file(dest_path);
    elapsed
}

// 2. Direct Play RAM Cache with 64 MiB limit
fn run_rust_direct_play_seq(base_dir: &Path) -> Duration {
    let info = create_bench_info();
    let hash = InfoHash([0u8; 20]);
    let st = DocumentTorrentStorage::new(Some(info), hash, base_dir);

    st.set_cache_limit(64 * 1024 * 1024);
    st.attach_direct_play(&[None], &[0])
        .expect("attach direct play");

    let chunk = vec![0xCCu8; BENCH_CHUNK_SIZE];
    let mut read_buf = vec![0u8; BENCH_CHUNK_SIZE];

    let start = Instant::now();
    for p in 0..BENCH_NUM_PIECES {
        let piece = st.piece_at(p);
        for c in 0..BENCH_CHUNKS_PER_PIECE {
            piece
                .write_at(&chunk, (c * BENCH_CHUNK_SIZE) as i64)
                .expect("write chunk");
        }
        piece.mark_complete().expect("mark complete");
        for c in 0..BENCH_CHUNKS_PER_PIECE {
            piece
                .read_at(&mut read_buf, (c * BENCH_CHUNK_SIZE) as i64)
                .expect("read chunk");
        }
    }
    let elapsed = start.elapsed();
    let _ = TorrentStorage::close(&*st);
    elapsed
}

fn run_rust_direct_play_random_seeks(base_dir: &Path) -> Duration {
    let info = create_bench_info();
    let hash = InfoHash([0u8; 20]);
    let st = DocumentTorrentStorage::new(Some(info), hash, base_dir);

    st.set_cache_limit(64 * 1024 * 1024);
    st.attach_direct_play(&[None], &[0])
        .expect("attach direct play");

    let chunk = vec![0xDDu8; BENCH_CHUNK_SIZE];
    let mut read_buf = vec![0u8; BENCH_CHUNK_SIZE];

    let num_seeks = 32;
    let pieces_per_seek = 8;
    let mut state: u64 = 12345;

    let start = Instant::now();
    for _ in 0..num_seeks {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let start_piece = ((state >> 33) as usize) % (BENCH_NUM_PIECES - pieces_per_seek + 1);
        for p in start_piece..(start_piece + pieces_per_seek) {
            let piece = st.piece_at(p);
            for c in 0..BENCH_CHUNKS_PER_PIECE {
                piece
                    .write_at(&chunk, (c * BENCH_CHUNK_SIZE) as i64)
                    .expect("write chunk");
            }
            piece.mark_complete().expect("mark complete");
            for c in 0..BENCH_CHUNKS_PER_PIECE {
                piece
                    .read_at(&mut read_buf, (c * BENCH_CHUNK_SIZE) as i64)
                    .expect("read chunk");
            }
        }
    }
    let elapsed = start.elapsed();
    let _ = TorrentStorage::close(&*st);
    elapsed
}

// 3. Checkpoint Save/Restore of 256 MiB torrent
fn run_rust_checkpoint_save(base_dir: &Path, iters: usize) -> Duration {
    let chk_path = base_dir.join("test_save.chk");
    let mut complete = vec![false; BENCH_NUM_PIECES];
    for i in (0..BENCH_NUM_PIECES).step_by(2) {
        complete[i] = true;
    }

    let cp = PieceCheckpoint {
        info_hash: [1, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        piece_length: BENCH_PIECE_LENGTH,
        num_pieces: BENCH_NUM_PIECES,
        files: vec![FileIdentity {
            external: true,
            dev: 100,
            ino: 200,
            length: BENCH_TORRENT_BYTES,
            mod_nano: 1_000_000_000,
            path: "bench-torrent.bin".into(),
        }],
        complete,
    };

    let start = Instant::now();
    for _ in 0..iters {
        write_piece_checkpoint(&chk_path, &cp).expect("write checkpoint");
    }
    let elapsed = start.elapsed();
    let _ = fs::remove_file(chk_path);
    elapsed
}

fn run_rust_checkpoint_restore(base_dir: &Path, iters: usize) -> Duration {
    let chk_path = base_dir.join("test_restore.chk");
    let mut complete = vec![false; BENCH_NUM_PIECES];
    for i in (0..BENCH_NUM_PIECES).step_by(2) {
        complete[i] = true;
    }

    let cp = PieceCheckpoint {
        info_hash: [1, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        piece_length: BENCH_PIECE_LENGTH,
        num_pieces: BENCH_NUM_PIECES,
        files: vec![FileIdentity {
            external: true,
            dev: 100,
            ino: 200,
            length: BENCH_TORRENT_BYTES,
            mod_nano: 1_000_000_000,
            path: "bench-torrent.bin".into(),
        }],
        complete,
    };

    write_piece_checkpoint(&chk_path, &cp).expect("write checkpoint");

    let start = Instant::now();
    for _ in 0..iters {
        let _ = read_piece_checkpoint(&chk_path).expect("read checkpoint");
    }
    let elapsed = start.elapsed();
    let _ = fs::remove_file(chk_path);
    elapsed
}

// 4. Piece Aggregation for /pieces heat grid on 10,000 pieces
fn run_rust_piece_aggregation(iters: usize) -> Duration {
    const TOTAL_PIECES: usize = 10_000;
    const MAX_BUCKETS: usize = 100;

    let num_runs = 500;
    let per_run = (TOTAL_PIECES / num_runs) as u32;
    let mut runs = Vec::with_capacity(num_runs);
    for i in 0..num_runs {
        runs.push(PieceRun {
            length: per_run,
            flags: (i % 3) as u32,
        });
    }

    let mut ranges = Vec::with_capacity(100);
    for i in 0..100 {
        let s = (i * 100) as u32;
        ranges.push(PieceRange {
            start: s,
            end: s + 80,
        });
    }

    let start = Instant::now();
    for _ in 0..iters {
        let _ = aggregate_piece_buckets(&runs, &ranges, TOTAL_PIECES as u32, MAX_BUCKETS as u32)
            .expect("aggregate piece buckets");
    }
    start.elapsed()
}

fn main() {
    let fixture_dir = PathBuf::from("/home/kar/.cache/wt-tmp/perf/storage-rust");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fixture dir");

    println!("=== RUST STORAGE BENCHMARKS ===");

    let measure = |name: &str, bytes: i64, f: &mut dyn FnMut() -> Duration| {
        if std::env::var("WT_STORAGE_BENCH_FILTER").is_ok_and(|filter| !name.contains(&filter)) {
            return;
        }
        let bytes_before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let count_before = ALLOC_COUNT.load(Ordering::Relaxed);

        let dur = f();

        let bytes_after = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let count_after = ALLOC_COUNT.load(Ordering::Relaxed);
        let rss = get_peak_rss_mib();

        let sec = dur.as_secs_f64();
        let mibs = (bytes as f64 / (1024.0 * 1024.0)) / sec;
        let alloc_kb = (bytes_after - bytes_before) as f64 / 1024.0;
        let num_allocs = count_after - count_before;

        println!(
            "[RUST BENCH] {:<30} | {:8.2} MiB/s | {:8.2} ms | {:8.1} KB alloc | {:6} allocs | RSS: {:6.1} MiB",
            name,
            mibs,
            sec * 1000.0,
            alloc_kb,
            num_allocs,
            rss
        );
    };

    // 1. Download Write-Back
    measure(
        "Download_Write_Seq_256MiB",
        BENCH_TORRENT_BYTES,
        &mut || run_rust_download_write_seq(&fixture_dir),
    );
    measure("Download_Read_Seq_256MiB", BENCH_TORRENT_BYTES, &mut || {
        run_rust_download_read_seq(&fixture_dir)
    });
    measure(
        "Download_Write_Rand_256MiB",
        BENCH_TORRENT_BYTES,
        &mut || run_rust_download_write_random(&fixture_dir),
    );
    measure(
        "Download_Read_Rand_256MiB",
        BENCH_TORRENT_BYTES,
        &mut || run_rust_download_read_random(&fixture_dir),
    );

    // 2. Direct Play
    measure("DirectPlay_Seq_256MiB", BENCH_TORRENT_BYTES, &mut || {
        run_rust_direct_play_seq(&fixture_dir)
    });
    measure(
        "DirectPlay_RandSeek_256MiB",
        BENCH_TORRENT_BYTES,
        &mut || run_rust_direct_play_random_seeks(&fixture_dir),
    );

    // 3. Checkpoint (2000 iters)
    let chk_iters = 2000;
    measure("Checkpoint_Save_2k", (chk_iters * 1024) as i64, &mut || {
        run_rust_checkpoint_save(&fixture_dir, chk_iters)
    });
    measure(
        "Checkpoint_Restore_2k",
        (chk_iters * 1024) as i64,
        &mut || run_rust_checkpoint_restore(&fixture_dir, chk_iters),
    );

    // 4. Piece Aggregation (5000 iters)
    let agg_iters = 5000;
    measure(
        "Piece_Aggregation_5k",
        (agg_iters * 10000) as i64,
        &mut || run_rust_piece_aggregation(agg_iters),
    );

    let _ = fs::remove_dir_all(&fixture_dir);
}
