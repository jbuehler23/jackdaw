use std::io::{BufRead, BufReader, Write as _};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use bevy::{remote::BrpRequest, tasks::IoTaskPool};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Send a BRP JSON-RPC 2.0 request and return the result as a background task.
///
/// BRP is plain HTTP to a game on this machine, so this speaks just enough
/// HTTP/1.1 over a `TcpStream` on its own thread instead of pulling in an
/// HTTP client with TLS.
pub fn brp_request(
    endpoint: &str,
    method: &str,
    params: Option<serde_json::Value>,
) -> bevy::tasks::Task<Result<serde_json::Value, anyhow::Error>> {
    let req = BrpRequest {
        method: String::from(method),
        id: None,
        params,
    };
    let url = endpoint.to_string();

    let (sender, receiver) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("brp-request".into())
        .spawn(move || {
            let result = serde_json::to_vec(&req)
                .map_err(anyhow::Error::from)
                .and_then(|body| post_json(&url, &body));
            let _ = sender.send_blocking(result);
        });

    IoTaskPool::get().spawn(async move {
        spawned.map_err(|error| anyhow!("cannot start the BRP request: {error}"))?;
        let body = receiver
            .recv()
            .await
            .map_err(|_| anyhow!("BRP request ended without a reply"))??;
        let mut v: serde_json::Value = serde_json::from_slice(&body)?;

        if let Some(val) = v.get_mut("result") {
            Ok(val.take())
        } else if let Some(error) = v.get("error") {
            Err(anyhow!("BRP error: {error}"))
        } else {
            Err(anyhow!(
                "BRP error: Response returned neither 'result' nor 'error' field"
            ))
        }
    })
}

/// POST `body` as JSON to an `http://host:port/path` URL and return the
/// body of a 2xx reply.
fn post_json(url: &str, body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("{url}: only http:// endpoints are supported"))?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    };
    let has_port = authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    let address = if has_port {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let socket = address
        .to_socket_addrs()
        .with_context(|| format!("{url}: cannot resolve host"))?
        .next()
        .ok_or_else(|| anyhow!("{url}: host has no address"))?;

    let mut stream = TcpStream::connect_timeout(&socket, CONNECT_TIMEOUT)
        .with_context(|| format!("{url}: cannot connect"))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;

    read_response(BufReader::new(stream)).with_context(|| format!("{url}: bad HTTP response"))
}

/// Read an HTTP/1.1 response, returning the body of a 2xx status.
fn read_response(mut reader: impl BufRead) -> anyhow::Result<Vec<u8>> {
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| anyhow!("malformed status line {:?}", status_line.trim_end()))?;

    let mut content_length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            bail!("connection closed inside the headers");
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(value.parse::<usize>()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }

    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size_line = String::new();
            reader.read_line(&mut size_line)?;
            let size = size_line.trim().split(';').next().unwrap_or_default();
            let size = usize::from_str_radix(size, 16)
                .map_err(|_| anyhow!("malformed chunk size {:?}", size_line.trim_end()))?;
            if size == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..])?;
            let mut crlf = [0; 2];
            reader.read_exact(&mut crlf)?;
        }
    } else if let Some(length) = content_length {
        body.resize(length, 0);
        reader.read_exact(&mut body)?;
    } else {
        reader.read_to_end(&mut body)?;
    }

    if !(200..300).contains(&status) {
        let excerpt: String = String::from_utf8_lossy(&body).chars().take(200).collect();
        bail!("HTTP {status}: {excerpt}");
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    #[test]
    fn reads_a_content_length_body() {
        let response = b"HTTP/1.1 200 OK\r\ncontent-length: 12\r\n\r\n{\"result\":1}";
        assert_eq!(read_response(&response[..]).unwrap(), b"{\"result\":1}");
    }

    #[test]
    fn reads_a_chunked_body() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                         4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(read_response(&response[..]).unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn rejects_an_error_status() {
        let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope";
        assert!(read_response(&response[..]).is_err());
    }

    #[test]
    fn posts_to_a_local_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim_end().is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let reply = br#"{"jsonrpc":"2.0","result":"ok"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                reply.len()
            )
            .unwrap();
            stream.write_all(reply).unwrap();
            (request_line, body)
        });

        let reply = post_json(&format!("http://127.0.0.1:{port}"), b"{\"method\":\"m\"}").unwrap();
        let (request_line, body) = server.join().unwrap();
        assert_eq!(request_line, "POST / HTTP/1.1\r\n");
        assert_eq!(body, b"{\"method\":\"m\"}");
        assert_eq!(reply, br#"{"jsonrpc":"2.0","result":"ok"}"#);
    }
}
