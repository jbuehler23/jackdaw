//! Downloads over https.

use std::io::Read as _;

/// Fetch `url` and return the response body.
///
/// Only `https://` is accepted: plain http would let anyone on the network
/// choose the bytes, and nothing `jd` downloads has a reason to be served
/// that way.
pub fn fetch_https(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!(
            "{url}: refusing to fetch over plain http; use https"
        ));
    }
    let response = match ureq::get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            return Err(format!(
                "{url}: server returned {code} {}",
                response.status_text()
            ));
        }
        // A transport error already names the URL.
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{url}: {error}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_refused_before_connecting() {
        let error = fetch_https("http://127.0.0.1:1/bundle.jdext").unwrap_err();
        assert!(error.contains("use https"), "{error}");
    }
}
