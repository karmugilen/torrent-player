//! WebRTC peer connection + data channel as AsyncRead/AsyncWrite.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use super::WebTorrentError;

pub const DATA_CHANNEL_LABEL: &str = "webrtc-datachannel";

/// Go default outstanding offer budget (bounded resources).
pub const DEFAULT_MAX_OUTSTANDING_OFFERS: usize = 10;

#[derive(Clone, Debug)]
pub struct IceServer {
    pub urls: Vec<String>,
}

impl From<IceServer> for RTCIceServer {
    fn from(s: IceServer) -> Self {
        RTCIceServer {
            urls: s.urls,
            ..Default::default()
        }
    }
}

/// STUN servers from `engine-go/server.go`.
pub fn default_ice_servers() -> Vec<IceServer> {
    vec![IceServer {
        urls: vec![
            "stun:stun.l.google.com:19302".into(),
            "stun:stun1.l.google.com:19302".into(),
            "stun:stun2.l.google.com:19302".into(),
            "stun:stun3.l.google.com:19302".into(),
            "stun:stun4.l.google.com:19302".into(),
            "stun:global.stun.twilio.com:3478".into(),
            "stun:stun.cloudflare.com:3478".into(),
        ],
    }]
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PeerConnStats {
    pub dials: i64,
    pub converted_inbound_conns: i64,
    pub converted_outbound_conns: i64,
}

#[derive(Clone, Debug)]
pub struct SessionDescription {
    pub sdp_type: String,
    pub sdp: String,
}

/// An established data channel wrapped as a byte stream.
pub struct DataChannelStream {
    dc: Arc<RTCDataChannel>,
    incoming: mpsc::UnboundedReceiver<Bytes>,
    outgoing: mpsc::UnboundedSender<Bytes>,
    read_buf: BytesMut,
    closed: bool,
}

impl DataChannelStream {
    pub async fn close(&self) -> Result<(), WebTorrentError> {
        self.dc
            .close()
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))
    }
}

impl AsyncRead for DataChannelStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.closed {
            return Poll::Ready(Ok(()));
        }
        while self.read_buf.is_empty() {
            match self.incoming.poll_recv(cx) {
                Poll::Ready(Some(b)) => self.read_buf.extend_from_slice(&b),
                Poll::Ready(None) => {
                    self.closed = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        let n = buf.remaining().min(self.read_buf.len());
        buf.put_slice(&self.read_buf.split_to(n));
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for DataChannelStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.closed {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed",
            )));
        }
        let data = Bytes::copy_from_slice(buf);
        let n = data.len();
        if self.outgoing.send(data).is_err() {
            return Poll::Ready(Err(std::io::Error::other("writer closed")));
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.closed = true;
        let dc = Arc::clone(&self.dc);
        tokio::spawn(async move {
            let _ = dc.close().await;
        });
        Poll::Ready(Ok(()))
    }
}

pub struct Transport {
    pc: Arc<RTCPeerConnection>,
    dc: Option<Arc<RTCDataChannel>>,
}

impl Transport {
    async fn new_api() -> Result<webrtc::api::API, WebTorrentError> {
        crate::ensure_rustls_provider();
        let mut media = MediaEngine::default();
        media
            .register_default_codecs()
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media)
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        Ok(APIBuilder::new()
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .build())
    }

    fn config(ice: &[IceServer]) -> RTCConfiguration {
        RTCConfiguration {
            ice_servers: ice.iter().cloned().map(Into::into).collect(),
            ..Default::default()
        }
    }

    pub async fn create_offer(
        ice: &[IceServer],
    ) -> Result<(Self, SessionDescription), WebTorrentError> {
        let api = Self::new_api().await?;
        let pc = api
            .new_peer_connection(Self::config(ice))
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let pc = Arc::new(pc);

        let dc = pc
            .create_data_channel(DATA_CHANNEL_LABEL, None)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;

        let offer = pc
            .create_offer(None)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let mut gather = pc.gathering_complete_promise().await;
        pc.set_local_description(offer)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let _ = gather.recv().await;

        let local = pc
            .local_description()
            .await
            .ok_or_else(|| WebTorrentError::Webrtc("no local description".into()))?;

        Ok((
            Self { pc, dc: Some(dc) },
            SessionDescription {
                sdp_type: local.sdp_type.to_string(),
                sdp: local.sdp,
            },
        ))
    }

    pub async fn accept_answer(
        &mut self,
        sdp_type: &str,
        sdp: &str,
    ) -> Result<DataChannelStream, WebTorrentError> {
        let dc = self
            .dc
            .clone()
            .ok_or_else(|| WebTorrentError::Webrtc("no data channel".into()))?;
        // Attach message handler before ICE completes so we never miss early data.
        let stream = attach_messages(Arc::clone(&dc));

        let answer = RTCSessionDescription::answer(sdp.to_string())
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let _ = sdp_type;
        self.pc
            .set_remote_description(answer)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;

        wait_dc_open(&dc).await?;
        Ok(stream)
    }

    pub async fn create_answer(
        ice: &[IceServer],
        sdp_type: &str,
        sdp: &str,
    ) -> Result<
        (
            Self,
            SessionDescription,
            oneshot::Receiver<DataChannelStream>,
        ),
        WebTorrentError,
    > {
        let api = Self::new_api().await?;
        let pc = api
            .new_peer_connection(Self::config(ice))
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let pc = Arc::new(pc);

        let (stream_tx, stream_rx) = oneshot::channel();
        let stream_tx = Arc::new(MutexOnce::new(Some(stream_tx)));

        {
            let stream_tx = Arc::clone(&stream_tx);
            pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let stream_tx = Arc::clone(&stream_tx);
                Box::pin(async move {
                    // Attach reader immediately; do not await (keeps PC event loop free).
                    let stream = attach_messages(Arc::clone(&dc));
                    let pending = Arc::new(MutexOnce::new(Some(stream)));
                    let deliver = {
                        let stream_tx = Arc::clone(&stream_tx);
                        let pending = Arc::clone(&pending);
                        move || {
                            if let (Some(tx), Some(stream)) = (stream_tx.take(), pending.take()) {
                                let _ = tx.send(stream);
                            }
                        }
                    };
                    {
                        let stream_tx = Arc::clone(&stream_tx);
                        let pending = Arc::clone(&pending);
                        dc.on_open(Box::new(move || {
                            let stream_tx = Arc::clone(&stream_tx);
                            let pending = Arc::clone(&pending);
                            Box::pin(async move {
                                if let (Some(tx), Some(stream)) = (stream_tx.take(), pending.take())
                                {
                                    let _ = tx.send(stream);
                                }
                            })
                        }));
                    }
                    if dc.ready_state()
                        == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
                    {
                        deliver();
                    }
                })
            }));
        }

        let _ = sdp_type;
        let offer = RTCSessionDescription::offer(sdp.to_string())
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        pc.set_remote_description(offer)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let mut gather = pc.gathering_complete_promise().await;
        pc.set_local_description(answer)
            .await
            .map_err(|e| WebTorrentError::Webrtc(e.to_string()))?;
        let _ = gather.recv().await;

        let local = pc
            .local_description()
            .await
            .ok_or_else(|| WebTorrentError::Webrtc("no local description".into()))?;

        // Keep PC alive until stream is delivered by holding it in Transport.
        let transport = Self { pc, dc: None };
        let _ = RTCPeerConnectionState::Connected; // silence unused import if any

        Ok((
            transport,
            SessionDescription {
                sdp_type: local.sdp_type.to_string(),
                sdp: local.sdp,
            },
            stream_rx,
        ))
    }
}

struct MutexOnce<T>(parking_lot::Mutex<Option<T>>);

impl<T> MutexOnce<T> {
    fn new(v: Option<T>) -> Self {
        Self(parking_lot::Mutex::new(v))
    }
    fn take(&self) -> Option<T> {
        self.0.lock().take()
    }
}

async fn wait_dc_open(dc: &Arc<RTCDataChannel>) -> Result<(), WebTorrentError> {
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(MutexOnce::new(Some(tx)));
    let tx2 = Arc::clone(&tx);
    dc.on_open(Box::new(move || {
        let tx2 = Arc::clone(&tx2);
        Box::pin(async move {
            if let Some(t) = tx2.take() {
                let _ = t.send(());
            }
        })
    }));
    // Already open?
    if dc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
        if let Some(t) = tx.take() {
            let _ = t.send(());
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), rx)
        .await
        .map_err(|_| WebTorrentError::Webrtc("datachannel open timeout".into()))?
        .map_err(|_| WebTorrentError::Webrtc("datachannel open cancelled".into()))
}

fn attach_messages(dc: Arc<RTCDataChannel>) -> DataChannelStream {
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Bytes>();
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let in_tx = in_tx.clone();
        Box::pin(async move {
            let _ = in_tx.send(msg.data);
        })
    }));
    let dc_writer = Arc::clone(&dc);
    tokio::spawn(async move {
        while let Some(data) = out_rx.recv().await {
            if dc_writer.send(&data).await.is_err() {
                break;
            }
        }
    });
    DataChannelStream {
        dc,
        incoming: in_rx,
        outgoing: out_tx,
        read_buf: BytesMut::new(),
        closed: false,
    }
}
