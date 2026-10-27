//! Small real HTTP tracker/list fixture, including request headers and cancellation.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct HttpServer {
    pub url: String,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl HttpServer {
    pub fn new(handler: impl Fn(String, TcpStream) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let handler = Arc::new(handler);
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let handler = handler.clone();
                        requests.push(std::thread::spawn(move || {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(3)))
                                .unwrap();
                            let mut request = Vec::new();
                            let mut byte = [0];
                            while !request.ends_with(b"\r\n\r\n") && request.len() < 65536 {
                                if stream.read(&mut byte).unwrap_or(0) == 0 {
                                    return;
                                }
                                request.push(byte[0]);
                            }
                            handler(String::from_utf8_lossy(&request).into_owned(), stream);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            }
            for request in requests {
                request.join().unwrap();
            }
        });
        Self {
            url,
            stopped,
            worker: Some(worker),
        }
    }
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}
pub fn respond(mut stream: TcpStream, status: u16, headers: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status} fixture\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
