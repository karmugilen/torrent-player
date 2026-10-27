use bytes::{Bytes, BytesMut};
use wt_wire::codec::{decode_message, FramedReader, FramedWriter};
use wt_wire::message::Message;

#[tokio::test]
async fn test_decode_short_piece_eof() {
    let (client, server) = tokio::io::duplex(1024);
    let mut writer = FramedWriter::new(client);
    let mut reader = FramedReader::with_max_length(server, 1 << 15);

    tokio::spawn(async move {
        let msg = Message::Piece {
            index: 0,
            begin: 0,
            piece: Bytes::copy_from_slice(&[42]),
        };
        writer.write_msg(&msg);
        writer.flush().await.unwrap();
    });

    let msg = reader.read_msg().await.unwrap().expect("expected message");
    match msg {
        Message::Piece {
            index,
            begin,
            piece,
        } => {
            assert_eq!(index, 0);
            assert_eq!(begin, 0);
            assert_eq!(piece.len(), 1);
            assert_eq!(piece[0], 42);
        }
        other => panic!("unexpected message: {other:?}"),
    }

    // Next read should be EOF
    assert!(reader.read_msg().await.unwrap().is_none());
}

#[tokio::test]
async fn test_decode_overlong_piece() {
    let mut buf = BytesMut::new();
    let msg = Message::Piece {
        index: 0,
        begin: 0,
        piece: Bytes::copy_from_slice(&[1, 2, 3]),
    };
    msg.encode(&mut buf);

    // Max length smaller than message length
    let res = decode_message(&mut buf, 8);
    assert!(res.is_err(), "expected error for overlong message");
}

#[test]
fn test_short_read_unused_bytes() {
    let mut buf = BytesMut::from(&b"\x00\x00\x00\x02\x00!"[..]);
    let err = decode_message(&mut buf, 10).unwrap_err();
    assert!(
        err.to_string()
            .contains("unused bytes in message type Choke"),
        "error was: {err}"
    );
}

#[tokio::test]
async fn test_unexpected_eof() {
    let truncated_inputs: [&[u8]; 4] = [
        b"\x00\x00\x00",     // Header truncated.
        b"\x00\x00\x00\x01", // Expecting 1 more byte.
        // Request with wrong length, and too short anyway.
        b"\x00\x00\x00\x06\x06\x00\x00\x00\x00\x00",
        // Request truncated.
        b"\x00\x00\x00\x0b\x06\x00\x00\x00\x00\x00",
    ];

    for input in truncated_inputs {
        let mut reader = FramedReader::new(input);
        let res = reader.read_msg().await;
        assert!(res.is_err(), "expected an error decoding {input:?}");
    }
}

#[test]
fn test_zero_copy_piece_decoding() {
    let piece_data = vec![7u8; 16384];
    let original_bytes = Bytes::copy_from_slice(&piece_data);

    let msg = Message::Piece {
        index: 10,
        begin: 16384,
        piece: original_bytes.clone(),
    };

    let mut buf = BytesMut::new();
    msg.encode(&mut buf);

    let decoded = decode_message(&mut buf, 32768).unwrap().unwrap();
    match decoded {
        Message::Piece {
            index,
            begin,
            piece,
        } => {
            assert_eq!(index, 10);
            assert_eq!(begin, 16384);
            assert_eq!(piece.len(), 16384);
            assert_eq!(&piece[..], &piece_data[..]);
        }
        _ => panic!("wrong message variant"),
    }
}
