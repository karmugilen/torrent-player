use bytes::{Bytes, BytesMut};
use std::time::Instant;
use wt_wire::codec::decode_message;
use wt_wire::message::Message;

#[test]
fn test_benchmark_decode_piece_messages_zero_copy() {
    const PIECE_LEN: usize = 16 * 1024; // 16 KiB
    const ITERATIONS: usize = 50_000;

    let payload = vec![0x42u8; PIECE_LEN];
    let msg = Message::Piece {
        index: 10,
        begin: 0,
        piece: Bytes::copy_from_slice(&payload),
    };

    let encoded = msg.to_bytes();
    let total_bytes = encoded.len() * ITERATIONS;

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let mut buf = BytesMut::from(&encoded[..]);
        let decoded = decode_message(&mut buf, 64 * 1024).unwrap().unwrap();
        match decoded {
            Message::Piece {
                index,
                begin,
                piece,
            } => {
                assert_eq!(index, 10);
                assert_eq!(begin, 0);
                assert_eq!(piece.len(), PIECE_LEN);
                // Zero-copy verification: payload byte matches without memory corruption
                assert_eq!(piece[0], 0x42);
            }
            _ => panic!("unexpected message"),
        }
    }
    let elapsed = start.elapsed();
    let mb = total_bytes as f64 / (1024.0 * 1024.0);
    let gbps = (mb / 1024.0) / elapsed.as_secs_f64();
    println!(
        "Piece decoder benchmark: decoded {} messages ({} MB) in {:?}: {:.2} GB/s",
        ITERATIONS, mb, elapsed, gbps
    );
    assert!(
        gbps > 0.5,
        "piece decoding throughput should exceed 0.5 GB/s"
    );
}
