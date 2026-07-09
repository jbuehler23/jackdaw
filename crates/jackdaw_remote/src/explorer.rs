//! Static file server for the explorer web app.
//!
//! Serves the compile-time embedded `explorer_dist/` bundle over plain
//! HTTP/1.1 GET on a dedicated thread. Localhost tooling traffic only, so
//! a minimal hand-rolled server keeps the crate free of async runtimes.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use include_dir::{Dir, include_dir};

/// Default port for the explorer static server. BRP itself stays on 15702.
pub const DEFAULT_EXPLORER_PORT: u16 = 15703;

static EXPLORER_DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/explorer_dist");

/// Start the explorer static server on `port` (0 picks an ephemeral port).
/// Returns the port actually bound. The accept loop runs on a detached
/// thread for the lifetime of the process.
pub fn start_explorer_server(port: u16) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let bound_port = listener.local_addr()?.port();
    std::thread::Builder::new()
        .name("jackdaw-explorer-http".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = handle_connection(stream);
                });
            }
        })?;
    Ok(bound_port)
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut request_line = request.lines().next().unwrap_or("").split_whitespace();
    let method = request_line.next().unwrap_or("");
    let raw_path = request_line.next().unwrap_or("/");

    if method != "GET" {
        return write_response(&mut stream, 405, "text/plain", b"method not allowed");
    }

    let path = raw_path.split('?').next().unwrap_or("/");
    let file_path = path.trim_start_matches('/');

    if let Some(file) = EXPLORER_DIST.get_file(file_path) {
        return write_response(&mut stream, 200, mime_for(file_path), file.contents());
    }
    if file_path.is_empty() || !file_path.contains('.') {
        // Extensionless paths are client-side routes; serve the app shell.
        if let Some(index) = EXPLORER_DIST.get_file("index.html") {
            return write_response(&mut stream, 200, "text/html", index.contents());
        }
    }
    write_response(&mut stream, 404, "text/plain", b"not found")
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html",
        "js" => "text/javascript",
        "css" => "text/css",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}
