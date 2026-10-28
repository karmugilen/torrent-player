//! Two in-process WebRTC peers exchange 4 MiB over a data channel.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use wt_discovery::webtorrent::Transport;

async fn new_api() -> webrtc::api::API {
    wt_discovery::ensure_rustls_provider();
    let mut media = MediaEngine::default();
    media.register_default_codecs().unwrap();
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media).unwrap();
    APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build()
}

/// Direct webrtc-rs signal_pair-style smoke (proves ICE works in this env).
#[tokio::test]
async fn raw_webrtc_ping() {
    let api = new_api().await;
    let offer_pc = api
        .new_peer_connection(RTCConfiguration::default())
        .await
        .unwrap();
    let answer_pc = api
        .new_peer_connection(RTCConfiguration::default())
        .await
        .unwrap();

    let (opened_tx, mut opened_rx) = mpsc::channel::<()>(1);
    let opened_tx = Arc::new(tokio::sync::Mutex::new(Some(opened_tx)));
    answer_pc.on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
        let opened_tx = Arc::clone(&opened_tx);
        Box::pin(async move {
            d.on_open(Box::new(move || {
                let opened_tx = Arc::clone(&opened_tx);
                Box::pin(async move {
                    if let Some(tx) = opened_tx.lock().await.take() {
                        let _ = tx.send(()).await;
                    }
                })
            }));
        })
    }));

    let _dc = offer_pc
        .create_data_channel("webrtc-datachannel", None)
        .await
        .unwrap();

    let offer = offer_pc.create_offer(None).await.unwrap();
    let mut og = offer_pc.gathering_complete_promise().await;
    offer_pc.set_local_description(offer).await.unwrap();
    let _ = og.recv().await;

    answer_pc
        .set_remote_description(offer_pc.local_description().await.unwrap())
        .await
        .unwrap();
    let answer = answer_pc.create_answer(None).await.unwrap();
    let mut ag = answer_pc.gathering_complete_promise().await;
    answer_pc.set_local_description(answer).await.unwrap();
    let _ = ag.recv().await;

    offer_pc
        .set_remote_description(answer_pc.local_description().await.unwrap())
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(15), opened_rx.recv())
        .await
        .expect("dc open timeout")
        .expect("dc open closed");

    offer_pc.close().await.ok();
    answer_pc.close().await.ok();
}

async fn connect_pair() -> (
    wt_discovery::webtorrent::DataChannelStream,
    wt_discovery::webtorrent::DataChannelStream,
    Transport,
) {
    let ice = vec![];
    let (mut offerer, offer_sdp) = Transport::create_offer(&ice).await.expect("offer");
    let (answerer, answer_sdp, answer_stream_rx) =
        Transport::create_answer(&ice, &offer_sdp.sdp_type, &offer_sdp.sdp)
            .await
            .expect("answer");

    let offer_accept = tokio::spawn(async move {
        offerer
            .accept_answer(&answer_sdp.sdp_type, &answer_sdp.sdp)
            .await
            .expect("accept answer")
    });

    let answer_wait = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(20), answer_stream_rx)
            .await
            .expect("answer dc timeout")
            .expect("answer dc cancelled")
    });

    let (offer_stream, answer_stream) = tokio::join!(offer_accept, answer_wait);
    (
        offer_stream.expect("offer join"),
        answer_stream.expect("answer join"),
        answerer,
    )
}

#[tokio::test]
async fn exchange_small_ping() {
    let (mut offer_stream, mut answer_stream, _answerer) = connect_pair().await;
    offer_stream.write_all(b"ping").await.expect("write");
    let mut buf = [0u8; 4];
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        answer_stream.read_exact(&mut buf),
    )
    .await
    .expect("read timeout")
    .expect("read");
    assert_eq!(&buf, b"ping");
}

#[tokio::test]
async fn exchange_4mib() {
    let (mut offer_stream, mut answer_stream, _answerer) = connect_pair().await;

    const N: usize = 4 * 1024 * 1024;
    let payload = vec![0xABu8; N];

    let writer = tokio::spawn(async move {
        const CHUNK: usize = 16 * 1024;
        for chunk in payload.chunks(CHUNK) {
            offer_stream.write_all(chunk).await.expect("write");
        }
        // Give the SCTP stack a moment to drain.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        offer_stream
    });

    let mut got = vec![0u8; N];
    let mut read = 0usize;
    while read < N {
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            answer_stream.read(&mut got[read..]),
        )
        .await
        .expect("read timeout")
        .expect("read");
        if n == 0 {
            break;
        }
        read += n;
    }
    assert_eq!(read, N, "short read");
    assert!(got.iter().all(|&b| b == 0xAB));
    let _ = writer.await;
}

/// Public WebTorrent peers can prefer AES-256 SRTP even for data-only sessions.
/// webrtc-srtp 0.14 derives that 32-byte key with Aes128 and panics.
#[tokio::test]
async fn peer_preferring_aes256_srtp_falls_back_without_panicking() {
    use webrtc::api::setting_engine::SettingEngine;
    use webrtc::dtls::extension::extension_use_srtp::SrtpProtectionProfile;

    let mut settings = SettingEngine::default();
    settings.set_srtp_protection_profiles(vec![
        SrtpProtectionProfile::Srtp_Aead_Aes_256_Gcm,
        SrtpProtectionProfile::Srtp_Aead_Aes_128_Gcm,
    ]);
    wt_discovery::ensure_rustls_provider();
    let api = APIBuilder::new().with_setting_engine(settings).build();
    let remote = api.new_peer_connection(RTCConfiguration::default()).await.unwrap();
    let (tx, mut rx) = mpsc::channel(1);
    remote.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let tx = tx.clone();
        Box::pin(async move {
            dc.on_message(Box::new(move |message| {
                let tx = tx.clone();
                Box::pin(async move { let _ = tx.send(message.data).await; })
            }));
        })
    }));
    let (mut local, offer) = Transport::create_offer(&[]).await.unwrap();
    let offer = webrtc::peer_connection::sdp::session_description::RTCSessionDescription::offer(offer.sdp).unwrap();
    remote.set_remote_description(offer).await.unwrap();
    let answer = remote.create_answer(None).await.unwrap();
    let mut gathering = remote.gathering_complete_promise().await;
    remote.set_local_description(answer).await.unwrap();
    let _ = gathering.recv().await;
    let answer = remote.local_description().await.unwrap();
    let connected = tokio::time::timeout(std::time::Duration::from_secs(10), local.accept_answer("answer", &answer.sdp)).await;
    if let Ok(Ok(mut stream)) = connected {
        stream.write_all(b"safe SRTP negotiation").await.unwrap();
        let data = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert_eq!(&data[..], b"safe SRTP negotiation");
    } else {
        remote.close().await.ok();
        panic!("peer preferring AES-256 failed to negotiate a supported SRTP profile");
    }
    remote.close().await.unwrap();
}
