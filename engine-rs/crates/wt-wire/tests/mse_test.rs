use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use wt_wire::mse::{
    default_crypto_selector, initiate_handshake, read_until, receive_handshake, suffix_match_len,
    CryptoMethod,
};

#[test]
fn test_suffix_match_len() {
    assert_eq!(suffix_match_len(b"hello", b"world"), 0);
    assert_eq!(suffix_match_len(b"hello", b"lo"), 2);
    assert_eq!(suffix_match_len(b"hello", b"llo"), 3);
    assert_eq!(suffix_match_len(b"hello", b"hell"), 0);
    assert_eq!(suffix_match_len(b"hello", b"helloooo!"), 5);
    assert_eq!(suffix_match_len(b"hello", b"lol!"), 2);
    assert_eq!(suffix_match_len(b"hello", b"mondo"), 0);
    assert_eq!(suffix_match_len(b"mongo", b"webscale"), 0);
    assert_eq!(suffix_match_len(b"sup", b"person"), 1);
}

#[tokio::test]
async fn test_read_until() {
    let mut data = &b"feakjfeafeafegbaabc00"[..];
    let res = read_until(&mut data, b"abc", 50).await;
    assert!(res.is_ok());
    assert_eq!(data, b"00");

    let mut data_fail = &b"feakjfeafeafegbaadc00"[..];
    let res_fail = read_until(&mut data_fail, b"abc", 50).await;
    assert!(res_fail.is_err());
}

async fn run_handshake_test(
    ia: Option<&'static [u8]>,
    a_data: &'static [u8],
    b_data: &'static [u8],
    provides: CryptoMethod,
    selector: fn(CryptoMethod) -> CryptoMethod,
) {
    let (a, b) = duplex(8192);

    let skey = b"yep".to_vec();
    let skey_copy = skey.clone();

    let a_task = tokio::spawn(async move {
        let (mut stream, method) = initiate_handshake(a, &skey_copy, ia, provides)
            .await
            .unwrap();
        assert_eq!(method, selector(provides));

        stream.write_all(a_data).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; b_data.len()];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf[..], b_data);
    });

    let b_task = tokio::spawn(async move {
        let candidates = vec![b"nope".to_vec(), b"yep".to_vec(), b"maybe".to_vec()];
        let mut res = receive_handshake(b, candidates, selector).await.unwrap();
        assert_eq!(res.secret_key, skey);
        assert_eq!(res.crypto_method, selector(provides));

        res.stream.write_all(b_data).await.unwrap();
        res.stream.flush().await.unwrap();

        let total_expected = ia.map_or(0, |s| s.len()) + a_data.len();
        let mut buf = vec![0u8; total_expected];
        res.stream.read_exact(&mut buf).await.unwrap();

        if let Some(ia_bytes) = ia {
            assert_eq!(&buf[..ia_bytes.len()], ia_bytes);
            assert_eq!(&buf[ia_bytes.len()..], a_data);
        } else {
            assert_eq!(&buf[..], a_data);
        }
    });

    let (r_a, r_b) = tokio::join!(a_task, b_task);
    r_a.unwrap();
    r_b.unwrap();
}

#[tokio::test]
async fn test_handshake_default_all_variants() {
    run_handshake_test(
        Some(b"jump the gun, "),
        b"hello world",
        b"yo dawg",
        CryptoMethod::ALL,
        default_crypto_selector,
    )
    .await;

    run_handshake_test(
        None,
        b"hello world",
        b"yo dawg",
        CryptoMethod::ALL,
        default_crypto_selector,
    )
    .await;

    run_handshake_test(
        Some(b""),
        b"hello world",
        b"yo dawg",
        CryptoMethod::ALL,
        default_crypto_selector,
    )
    .await;
}

#[tokio::test]
async fn test_handshake_rc4_enforced() {
    run_handshake_test(
        Some(b"initial payload"),
        b"client secret data",
        b"server encrypted response",
        CryptoMethod::RC4,
        |_| CryptoMethod::RC4,
    )
    .await;
}

#[tokio::test]
async fn test_handshake_plaintext_selector() {
    run_handshake_test(
        Some(b"jump the gun, "),
        b"hello world",
        b"yo dawg",
        CryptoMethod::ALL,
        |_| CryptoMethod::PLAINTEXT,
    )
    .await;
}

#[tokio::test]
async fn test_receive_random_data_fails() {
    let (mut client, server) = duplex(1024);
    tokio::spawn(async move {
        let random_bytes = vec![0x7fu8; 96 + 532 + 10];
        let _ = client.write_all(&random_bytes).await;
    });
    let candidates = vec![b"cats".to_vec()];
    let res = receive_handshake(server, candidates, default_crypto_selector).await;
    assert!(res.is_err());
}
