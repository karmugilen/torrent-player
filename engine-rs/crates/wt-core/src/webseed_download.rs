//! Bounded BEP 19 downloads driven by the torrent's live piece demand.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::task::JoinSet;
use wt_discovery::webseed::{Extent, WebSeedClient};
use wt_types::{PiecePriority, BLOCK_SIZE};

use crate::client::ClientInner;
use crate::peer_conn::PeerTorrent;
use crate::picker::{piece_req_index_range, req_index_from_piece_and_offset};
use crate::torrent::TorrentInner;

const MAX_FETCH_BYTES: i64 = 1 << 20;
const MAX_FETCHES: usize = 4;

pub(crate) async fn run(client: Weak<ClientInner>, torrent: Weak<TorrentInner>) {
    let mut sources = HashMap::new();
    let mut pending = HashSet::new();
    let mut jobs = JoinSet::new();
    let mut source_cursor = 0usize;
    loop {
        let (Some(client), Some(torrent)) = (client.upgrade(), torrent.upgrade()) else {
            break;
        };
        if client.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
            break;
        }
        while let Some(result) = jobs.try_join_next() {
            if let Ok((begin, end)) = result {
                for req in begin..end {
                    pending.remove(&req);
                }
            }
        }
        if !torrent.data_download_disallowed.load(Ordering::Relaxed) {
            torrent.start_hashing();
            if let Some(info) = torrent.info.read().clone() {
                for url in torrent.web_seeds.read().iter() {
                    if !sources.contains_key(url) {
                        if let Ok(mut source) = WebSeedClient::new(url, MAX_FETCHES) {
                            source.set_info((*info).clone());
                            sources.insert(url.clone(), source);
                        }
                    }
                }
                while jobs.len() < MAX_FETCHES && !sources.is_empty() {
                    let focus = torrent
                        .request_focus
                        .load(Ordering::Relaxed)
                        .saturating_sub(1)
                        .max(0)
                        / info.piece_length;
                    let next = {
                        let pieces = torrent.pieces.read();
                        let dirty = torrent.dirty_chunks.read();
                        pieces
                            .iter()
                            .filter(|p| {
                                !p.complete && !p.checking && p.priority > PiecePriority::None
                            })
                            .filter_map(|p| {
                                let (begin, end) = piece_req_index_range(
                                    p.index,
                                    info.piece_length,
                                    info.total_length(),
                                );
                                let first = (begin..end)
                                    .find(|r| !dirty.contains(r) && !pending.contains(r))?;
                                let blocks = (MAX_FETCH_BYTES / BLOCK_SIZE as i64).max(1) as u32;
                                let last = (first..end.min(first + blocks))
                                    .take_while(|r| !dirty.contains(r) && !pending.contains(r))
                                    .last()?
                                    + 1;
                                Some((
                                    p.priority,
                                    (p.index as i64 - focus).abs(),
                                    p.index,
                                    first - begin,
                                    first,
                                    last,
                                ))
                            })
                            .max_by_key(|(priority, distance, _, _, _, _)| {
                                (*priority, std::cmp::Reverse(*distance))
                            })
                    };
                    let Some((_, _, piece, block, begin, end)) = next else {
                        break;
                    };
                    pending.extend(begin..end);
                    let source = sources
                        .values()
                        .nth(source_cursor % sources.len())
                        .unwrap()
                        .clone();
                    source_cursor += 1;
                    let offset = block as i64 * BLOCK_SIZE as i64;
                    let extent = Extent {
                        start: piece as i64 * info.piece_length + offset,
                        length: ((end - begin) as i64 * BLOCK_SIZE as i64)
                            .min(info.piece(piece).length - offset),
                    };
                    let tor = Arc::downgrade(&torrent);
                    let cl = Arc::downgrade(&client);
                    jobs.spawn(async move {
                        let result = tokio::time::timeout(
                            Duration::from_secs(15),
                            source.fetch_extent(extent),
                        )
                        .await;
                        if let (Some(torrent), Some(client)) = (tor.upgrade(), cl.upgrade()) {
                            if let Ok(Ok(data)) = result {
                                let Some(delay) = client.download_limiter.reserve_delay(data.len())
                                else {
                                    return (begin, end);
                                };
                                if !delay.is_zero() {
                                    tokio::time::sleep(delay).await;
                                }
                                if !torrent.data_download_disallowed.load(Ordering::Relaxed)
                                    && !torrent.closed.load(Ordering::Relaxed)
                                    && torrent.piece_priority(piece) > PiecePriority::None
                                {
                                    for (i, chunk) in data.chunks(BLOCK_SIZE as usize).enumerate() {
                                        let at = offset as u32 + i as u32 * BLOCK_SIZE;
                                        if torrent.write_chunk(piece, at, chunk).is_err() {
                                            break;
                                        }
                                        let req = req_index_from_piece_and_offset(
                                            piece,
                                            at,
                                            torrent.piece_length(),
                                        );
                                        torrent.mark_chunk_dirty(req);
                                        torrent
                                            .webseed_stats
                                            .bytes_read_data
                                            .fetch_add(chunk.len() as i64, Ordering::Relaxed);
                                        torrent
                                            .webseed_stats
                                            .bytes_read_useful_data
                                            .fetch_add(chunk.len() as i64, Ordering::Relaxed);
                                    }
                                    if torrent.piece_all_dirty(piece) {
                                        torrent.queue_piece_hash(piece);
                                    }
                                }
                            } else {
                                // Failed sources back off without blocking peer I/O or other seeds.
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        }
                        (begin, end)
                    });
                }
            }
        } else {
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
            pending.clear();
        }
        drop(torrent);
        drop(client);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    jobs.abort_all();
}
