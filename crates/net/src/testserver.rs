//! A tiny loopback HTTP server for tests. Nothing here touches the real network.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum Reply {
    /// Write these bytes, then close.
    Raw(Vec<u8>),
    /// Accept and say nothing for a few seconds (a stalled server).
    Hang,
}

/// `HTTP/1.1` response with a length and `Connection: close`.
pub fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    let mut out = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len()).into_bytes();
    for (k, v) in headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    Reply::Raw(out)
}

pub struct Server {
    pub url: String,
    /// The text of every request received (request line, headers, body).
    pub requests: Arc<Mutex<Vec<String>>>,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Starts a server on a random port. `handler(n, request)` answers the n-th request (from 0).
pub fn serve(handler: impl Fn(usize, &str) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let handler = Arc::new(handler);
    let (r2, h2, s2) = (requests.clone(), hits.clone(), stop.clone());
    std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let n = h2.fetch_add(1, Ordering::SeqCst);
                    let (handler, requests) = (handler.clone(), r2.clone());
                    std::thread::spawn(move || handle(stream, n, &*handler, &requests));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Server { url: format!("http://127.0.0.1:{port}"), requests, hits, stop }
}

fn handle(mut stream: TcpStream, n: usize, handler: &(dyn Fn(usize, &str) -> Reply + Send + Sync), requests: &Mutex<Vec<String>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(k) = stream.read(&mut chunk) {
        if k == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..k]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let want =
                text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
            if data.len() >= end + 4 + want {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&data).into_owned();
    requests.lock().expect("lock").push(text.clone());
    match handler(n, &text) {
        Reply::Raw(bytes) => {
            let _ = stream.write_all(&bytes);
            let _ = stream.flush();
        }
        Reply::Hang => std::thread::sleep(Duration::from_secs(4)),
    }
}
