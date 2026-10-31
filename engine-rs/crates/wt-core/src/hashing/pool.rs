//! Bounded piece-hasher worker pool (Go `startPieceHashers` / `pieceHasher`).

use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use wt_types::{PieceIndex, PiecePriority, PieceStorage};

use super::smartban::{BannableAddr, SmartBanCache};
use super::verify::{hash_piece_sync, ExpectedHash};
use crate::CancelToken;

/// Default `ClientConfig::piece_hashers_per_torrent` (Go default 2).
pub const DEFAULT_HASHERS_PER_TORRENT: usize = 2;

/// Optional smart-ban range covering the piece's request indexes.
#[derive(Clone, Debug)]
pub struct SmartBanPieceRange {
    pub block_begin: u32,
    pub chunk_size: u32,
}

/// One piece to hash.
#[derive(Clone)]
pub struct HashJob {
    pub piece_index: PieceIndex,
    pub priority: PiecePriority,
    pub storage: Arc<dyn PieceStorage>,
    pub expected: ExpectedHash,
    /// Torrent `piece_length` (v2 layer pad).
    pub piece_length: i64,
    /// Actual bytes present in this piece.
    pub data_length: i64,
    /// Hybrid v1 zero-padding length.
    pub v1_zero_pad: i64,
    pub smartban: Option<SmartBanPieceRange>,
    /// Novel verify sequence / FIFO tie-break (lower first).
    pub seq: u64,
}

/// Outcome delivered on the result channel.
#[derive(Clone, Debug)]
pub struct HashResult {
    pub piece_index: PieceIndex,
    pub correct: bool,
    pub bytes_hashed: u64,
    pub error: Option<String>,
    pub differing_peers: Vec<BannableAddr>,
    pub seq: u64,
    pub cancelled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum HashQueueError {
    #[error("hasher pool shut down")]
    ShutDown,
    #[error("cancelled")]
    Cancelled,
}

struct HeapEntry {
    job: HashJob,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.job.piece_index == other.job.piece_index && self.job.seq == other.job.seq
    }
}
impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // BinaryHeap is max-heap: higher priority first, then lower index, then lower seq.
        match self.job.priority.cmp(&other.job.priority) {
            CmpOrdering::Equal => {}
            o => return o,
        }
        match other.job.piece_index.cmp(&self.job.piece_index) {
            CmpOrdering::Equal => {}
            o => return o,
        }
        other.job.seq.cmp(&self.job.seq)
    }
}

struct Shared {
    queue: Mutex<BinaryHeap<HeapEntry>>,
    cv: Condvar,
    shut_down: AtomicBool,
    cancel: Mutex<CancelToken>,
    active: AtomicUsize,
    queued: AtomicUsize,
    smartban: Mutex<Option<Arc<SmartBanCache>>>,
    result_tx: Mutex<Option<Sender<HashResult>>>,
}

/// Sticky per-torrent hasher pool. Workers pull highest-priority jobs.
pub struct PieceHasherPool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl PieceHasherPool {
    pub fn new(workers: usize, result_tx: Sender<HashResult>) -> Self {
        let workers = workers.max(1);
        let shared = Arc::new(Shared {
            queue: Mutex::new(BinaryHeap::new()),
            cv: Condvar::new(),
            shut_down: AtomicBool::new(false),
            cancel: Mutex::new(CancelToken::default()),
            active: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            smartban: Mutex::new(None),
            result_tx: Mutex::new(Some(result_tx)),
        });
        let mut handles = Vec::with_capacity(workers);
        for id in 0..workers {
            let shared = Arc::clone(&shared);
            handles.push(
                std::thread::Builder::new()
                    .name(format!("wt-piece-hash-{id}"))
                    .spawn(move || worker_loop(shared))
                    .expect("spawn piece hasher"),
            );
        }
        Self {
            shared,
            workers: handles,
        }
    }

    /// Channel constructor: returns `(pool, result_rx)`.
    pub fn with_workers(workers: usize) -> (Self, Receiver<HashResult>) {
        let (tx, rx) = mpsc::channel();
        (Self::new(workers, tx), rx)
    }


    pub fn set_cancel_token(&self, token: CancelToken) {
        *self.shared.cancel.lock() = token;
        self.shared.cv.notify_all();
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.shared.cancel.lock().clone()
    }

    /// Wake idle workers (call after `CancelToken::cancel()` so queued jobs drain).
    pub fn wake(&self) {
        self.shared.cv.notify_all();
    }

    pub fn queue(&self, job: HashJob) -> Result<(), HashQueueError> {
        if self.shared.shut_down.load(Ordering::SeqCst) {
            return Err(HashQueueError::ShutDown);
        }
        if self.shared.cancel.lock().is_cancelled() {
            return Err(HashQueueError::Cancelled);
        }
        {
            let mut q = self.shared.queue.lock();
            q.push(HeapEntry { job });
            self.shared.queued.fetch_add(1, Ordering::SeqCst);
        }
        self.shared.cv.notify_one();
        Ok(())
    }



    pub fn shutdown(&self) {
        self.shared.shut_down.store(true, Ordering::SeqCst);
        self.shared.cv.notify_all();
    }
}

impl Drop for PieceHasherPool {
    fn drop(&mut self) {
        self.shutdown();
        // Detach workers: joining here can deadlock if drop runs from a worker.
        for h in self.workers.drain(..) {
            let _ = h.join();
        }
        *self.shared.result_tx.lock() = None;
    }
}

/// Queueing a job and shutting down both notify; this is only a safety net. A
/// short wait here woke every worker of every torrent all day with no work.
const IDLE_WAIT: Duration = Duration::from_secs(5);

fn worker_loop(shared: Arc<Shared>) {
    // Reusable buffer — no per-piece allocation churn.
    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    loop {
        let job = {
            let mut q = shared.queue.lock();
            loop {
                if shared.shut_down.load(Ordering::SeqCst) {
                    return;
                }
                if shared.cancel.lock().is_cancelled() {
                    // Drain queue as cancelled.
                    while let Some(entry) = q.pop() {
                        shared.queued.fetch_sub(1, Ordering::SeqCst);
                        emit_cancelled(&shared, entry.job);
                    }
                    let _ = shared.cv.wait_for(&mut q, IDLE_WAIT);
                    continue;
                }
                if let Some(entry) = q.pop() {
                    shared.queued.fetch_sub(1, Ordering::SeqCst);
                    break entry.job;
                }
                let _ = shared.cv.wait_for(&mut q, IDLE_WAIT);
            }
        };

        if shared.cancel.lock().is_cancelled() {
            emit_cancelled(&shared, job);
            continue;
        }

        shared.active.fetch_add(1, Ordering::SeqCst);
        let result = run_job(&shared, &job, &mut buf);
        shared.active.fetch_sub(1, Ordering::SeqCst);
        if let Some(tx) = shared.result_tx.lock().as_ref() {
            let _ = tx.send(result);
        }
    }
}

fn emit_cancelled(shared: &Shared, job: HashJob) {
    if let Some(tx) = shared.result_tx.lock().as_ref() {
        let _ = tx.send(HashResult {
            piece_index: job.piece_index,
            correct: false,
            bytes_hashed: 0,
            error: None,
            differing_peers: Vec::new(),
            seq: job.seq,
            cancelled: true,
        });
    }
}

fn run_job(shared: &Shared, job: &HashJob, buf: &mut Vec<u8>) -> HashResult {
    if shared.cancel.lock().is_cancelled() {
        return HashResult {
            piece_index: job.piece_index,
            correct: false,
            bytes_hashed: 0,
            error: None,
            differing_peers: Vec::new(),
            seq: job.seq,
            cancelled: true,
        };
    }

    let smartban_cache = shared.smartban.lock().clone();
    let smartban_arg = match (&smartban_cache, &job.smartban) {
        (Some(cache), Some(range)) => Some((cache.as_ref(), range.block_begin, range.chunk_size)),
        _ => None,
    };

    match hash_piece_sync(
        &*job.storage,
        &job.expected,
        job.data_length,
        job.piece_length,
        job.v1_zero_pad,
        smartban_arg,
        buf,
    ) {
        Ok(outcome) => {
            if outcome.correct {
                if let (Some(cache), Some(range)) = (&smartban_cache, &job.smartban) {
                    let count = (job.data_length.max(0) as u32).div_ceil(range.chunk_size.max(1));
                    cache.forget_block_seq(range.block_begin, count);
                }
            }
            HashResult {
                piece_index: job.piece_index,
                correct: outcome.correct,
                bytes_hashed: outcome.bytes_hashed,
                error: None,
                differing_peers: outcome.differing_peers,
                seq: job.seq,
                cancelled: false,
            }
        }
        Err(e) => HashResult {
            piece_index: job.piece_index,
            correct: false,
            bytes_hashed: 0,
            error: Some(e.to_string()),
            differing_peers: Vec::new(),
            seq: job.seq,
            cancelled: matches!(e, super::verify::HashWorkError::Cancelled),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_adapter::MemoryPieceStorage;
    use sha1::{Digest, Sha1};
    use std::time::Duration;
    use wt_types::{Piece, PieceStorage};

    fn piece_storage(index: u32, data: &[u8]) -> (Arc<MemoryPieceStorage>, [u8; 20]) {
        let p = Piece {
            index,
            offset: index as i64 * data.len() as i64,
            length: data.len() as i64,
        };
        let s = Arc::new(MemoryPieceStorage::new(p, None));
        s.write_at(data, 0).unwrap();
        let mut h = Sha1::new();
        h.update(data);
        (s, h.finalize().into())
    }

    #[test]
    fn priority_orders_now_before_normal() {
        let (pool, rx) = PieceHasherPool::with_workers(1);
        // Queue Normal first, then Now — Now must complete first with 1 worker if both pending.
        let big = vec![7u8; 2 * 1024 * 1024];
        let (s_busy, h_busy) = piece_storage(100, &big);
        let (s_n, h_n) = piece_storage(2, b"normal-piece-data");
        let (s_now, h_now) = piece_storage(3, b"now-priority-piece");

        pool.queue(HashJob {
            piece_index: 100,
            priority: PiecePriority::None,
            storage: s_busy,
            expected: ExpectedHash::V1(h_busy),
            piece_length: big.len() as i64,
            data_length: big.len() as i64,
            v1_zero_pad: 0,
            smartban: None,
            seq: 0,
        })
        .unwrap();
        // Give worker time to start the busy job
        std::thread::sleep(Duration::from_millis(30));
        pool.queue(HashJob {
            piece_index: 2,
            priority: PiecePriority::Normal,
            storage: s_n,
            expected: ExpectedHash::V1(h_n),
            piece_length: 17,
            data_length: 17,
            v1_zero_pad: 0,
            smartban: None,
            seq: 1,
        })
        .unwrap();
        pool.queue(HashJob {
            piece_index: 3,
            priority: PiecePriority::Now,
            storage: s_now,
            expected: ExpectedHash::V1(h_now),
            piece_length: 18,
            data_length: 18,
            v1_zero_pad: 0,
            smartban: None,
            seq: 2,
        })
        .unwrap();

        let mut order = Vec::new();
        for _ in 0..3 {
            let r = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            order.push(r.piece_index);
        }
        // Busy (100) finishes first (already in flight), then Now (3), then Normal (2).
        assert_eq!(order[0], 100);
        assert_eq!(order[1], 3, "Now should hash before Normal; got {order:?}");
        assert_eq!(order[2], 2);
        pool.shutdown();
    }

    #[test]
    fn cancel_rejects_new_jobs() {
        let (pool, _rx) = PieceHasherPool::with_workers(1);
        let cancel = CancelToken::default();
        pool.set_cancel_token(cancel.clone());
        cancel.cancel();
        pool.wake();

        let data = b"x";
        let (s, h) = piece_storage(0, data);
        let err = pool
            .queue(HashJob {
                piece_index: 0,
                priority: PiecePriority::Normal,
                storage: s,
                expected: ExpectedHash::V1(h),
                piece_length: 1,
                data_length: 1,
                v1_zero_pad: 0,
                smartban: None,
                seq: 0,
            })
            .unwrap_err();
        assert!(matches!(err, HashQueueError::Cancelled));
        pool.shutdown();
    }
}
