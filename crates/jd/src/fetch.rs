//! Downloads over https, and reads of `file://` URLs for local mirrors.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use sha2::Digest as _;

/// Fetch `url` and return the response body.
///
/// Only `https://` is accepted: plain http would let anyone on the network
/// choose the bytes, and nothing `jd` downloads has a reason to be served
/// that way.
pub fn fetch_https(url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    open_https(url)?
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{url}: {error}"))?;
    Ok(bytes)
}

/// Fetch an `https://` or `file://` URL.
pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    match file_url_path(url) {
        Some(path) => std::fs::read(&path).map_err(|error| format!("{url}: {error}")),
        None => fetch_https(url),
    }
}

/// Stream an `https://` or `file://` URL into `dest`, returning the byte
/// count and the lowercase hex SHA-256 of what was written.
pub fn download(url: &str, dest: &Path) -> Result<(u64, String), String> {
    let mut reader: Box<dyn std::io::Read> = match file_url_path(url) {
        Some(path) => {
            Box::new(std::fs::File::open(&path).map_err(|error| format!("{url}: {error}"))?)
        }
        None => open_https(url)?,
    };
    let mut file =
        std::fs::File::create(dest).map_err(|error| format!("{}: {error}", dest.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut total = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("{url}: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|error| format!("{}: {error}", dest.display()))?;
        total += read as u64;
    }
    file.sync_all()
        .map_err(|error| format!("{}: {error}", dest.display()))?;
    Ok((total, jackdaw_releases::hex(&hasher.finalize())))
}

fn open_https(url: &str) -> Result<Box<dyn std::io::Read + Send + Sync>, String> {
    if !url.starts_with("https://") {
        return Err(format!(
            "{url}: refusing to fetch over plain http; use https"
        ));
    }
    match ureq::get(url).call() {
        Ok(response) => Ok(response.into_reader()),
        Err(ureq::Error::Status(code, response)) => Err(format!(
            "{url}: server returned {code} {}",
            response.status_text()
        )),
        // A transport error already names the URL.
        Err(error) => Err(error.to_string()),
    }
}

/// The local path a `file://` URL names.
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let windows_drive = bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':';
    Some(PathBuf::from(if windows_drive { &rest[1..] } else { rest }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_refused_before_connecting() {
        let error = fetch_https("http://127.0.0.1:1/bundle.jdext").unwrap_err();
        assert!(error.contains("use https"), "{error}");
        let error = fetch("http://127.0.0.1:1/releases.json").unwrap_err();
        assert!(error.contains("use https"), "{error}");
    }

    #[test]
    fn file_urls_read_and_hash_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundle.bin");
        std::fs::write(&source, b"bundle bytes").unwrap();
        let url = format!("file://{}", source.display());
        assert_eq!(fetch(&url).unwrap(), b"bundle bytes");

        let dest = dir.path().join("copy.bin");
        let (size, sha256) = download(&url, &dest).unwrap();
        assert_eq!(size, 12);
        assert_eq!(sha256, jackdaw_releases::sha256_hex(b"bundle bytes"));
        assert_eq!(std::fs::read(dest).unwrap(), b"bundle bytes");
    }

    #[test]
    fn windows_file_urls_drop_the_slash_before_the_drive() {
        assert_eq!(
            file_url_path("file:///C:/mirror/releases.json"),
            Some(PathBuf::from("C:/mirror/releases.json"))
        );
        assert_eq!(
            file_url_path("file:///srv/mirror/releases.json"),
            Some(PathBuf::from("/srv/mirror/releases.json"))
        );
        assert_eq!(file_url_path("https://example.com"), None);
    }
}
