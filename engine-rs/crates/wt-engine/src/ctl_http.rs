//! Localhost control HTTP server for the host binary.
//!
//! Mirrors the control listener started by Go `EngineServer.Start`: bind
//! `127.0.0.1:<port>` (fall back to an ephemeral port), forward every request to
//! [`EngineServer::request`], and treat `POST /shutdown` as graceful stop.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::server::EngineServer;

/// Handle for the control listener and its completion signal.
pub struct ControlServer {
    port: u16,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Resolves when `POST /shutdown` runs or [`ControlServer::stop`] is called.
    done_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl ControlServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Future that completes when the control server is done.
    pub fn take_done(&self) -> Option<oneshot::Receiver<()>> {
        self.done_rx.lock().take()
    }

    /// Stop accepting and resolve Done (idempotent).
    pub fn stop(&self) {
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }
    }
}

/// Serve control HTTP on `127.0.0.1:prefer_port` (or `:0` if that bind fails).
pub async fn serve(
    engine: Arc<EngineServer>,
    prefer_port: u16,
) -> std::io::Result<Arc<ControlServer>> {
    let listener = match TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], prefer_port))).await {
        Ok(l) => l,
        Err(_) => TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?,
    };
    let port = listener.local_addr()?.port();

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let done_tx = Arc::new(Mutex::new(Some(done_tx)));

    let server = Arc::new(ControlServer {
        port,
        shutdown_tx: Mutex::new(Some(shutdown_tx)),
        done_rx: Mutex::new(Some(done_rx)),
    });

    let engine_accept = engine.clone();
    let done_accept = done_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                res = listener.accept() => {
                    match res {
                        Ok((stream, _)) => {
                            let io = TokioIo::new(stream);
                            let engine = engine_accept.clone();
                            let done_tx = done_accept.clone();
                            tokio::spawn(async move {
                                let service = service_fn(move |req| {
                                    handle_control(req, engine.clone(), done_tx.clone())
                                });
                                let _ = Builder::new(TokioExecutor::new())
                                    .serve_connection(io, service)
                                    .await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        // Listener stopped: ensure Done is signalled.
        if let Some(tx) = done_accept.lock().take() {
            let _ = tx.send(());
        }
    });

    eprintln!(
        "webtor-rs control listening on http://127.0.0.1:{port} (streamPort={})",
        engine.stream_port.load(std::sync::atomic::Ordering::SeqCst)
    );
    Ok(server)
}

async fn handle_control(
    req: Request<Incoming>,
    engine: Arc<EngineServer>,
    done_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                br#"{"error":"failed to read body"}"#,
            ));
        }
    };
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    let is_shutdown = method == Method::POST.as_str()
        && (path == "/shutdown" || path == "shutdown" || path.ends_with("/shutdown"));

    // Control handlers may wait for magnet metadata or perform storage IO.
    // Keep runtime workers available to actually acquire that metadata.
    let worker = engine.clone();
    let (status, resp) =
        match tokio::task::spawn_blocking(move || worker.request(&method, &path, &body)).await {
            Ok(response) => response,
            Err(_) => {
                return Ok(json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    br#"{"error":"control request failed"}"#,
                ))
            }
        };
    let response = Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(resp)))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from_static(
                    br#"{"error":"response build failed"}"#,
                )))
                .expect("static response")
        });

    if is_shutdown {
        engine.close();
        if let Some(tx) = done_tx.lock().take() {
            let _ = tx.send(());
        }
    }

    Ok(response)
}

fn json_response(status: StatusCode, body: &'static [u8]) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from_static(body)))
        .expect("static json response")
}
