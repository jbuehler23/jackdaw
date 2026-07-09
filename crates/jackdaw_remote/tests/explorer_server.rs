use std::io::{Read, Write};
use std::net::TcpStream;

use jackdaw_remote::explorer::start_explorer_server;

fn http_get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    response
}

#[test]
fn serves_index_at_root() {
    let port = start_explorer_server(0).expect("server started");
    let response = http_get(port, "/");
    assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
    assert!(response.contains("Content-Type: text/html"), "got: {response}");
    assert!(response.contains("Jackdaw Explorer"), "got: {response}");
}

#[test]
fn unknown_asset_with_extension_is_404() {
    let port = start_explorer_server(0).expect("server started");
    let response = http_get(port, "/missing.js");
    assert!(response.starts_with("HTTP/1.1 404"), "got: {response}");
}

#[test]
fn unknown_route_falls_back_to_index() {
    // Client-side routing: extensionless paths get the app shell.
    let port = start_explorer_server(0).expect("server started");
    let response = http_get(port, "/entities");
    assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
    assert!(response.contains("Jackdaw Explorer"), "got: {response}");
}

#[test]
fn non_get_method_is_405() {
    let port = start_explorer_server(0).expect("server started");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(stream, "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    assert!(response.starts_with("HTTP/1.1 405"), "got: {response}");
}
