//! JSON-RPC calls to the editor's remote API.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long a call may wait on an editor stalled on the main thread.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// A connection to one editor's remote API on loopback.
pub struct Brp {
    agent: ureq::Agent,
    port: u16,
    url: String,
}

/// Why a call failed: the editor could not be reached, or it answered with
/// an error.
#[derive(Debug)]
pub enum CallError {
    Unreachable(String),
    Refused(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(why) => write!(f, "editor unreachable: {why}"),
            Self::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl Brp {
    pub fn new(port: u16) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(2))
                .build(),
            port,
            url: format!("http://127.0.0.1:{port}/"),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, CallError> {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response = match self
            .agent
            .post(&self.url)
            .timeout(timeout)
            .send_json(request)
        {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                return Err(CallError::Unreachable(transport.to_string()));
            }
        };
        let body: Value = response
            .into_json()
            .map_err(|err| CallError::Unreachable(format!("{method}: unreadable answer: {err}")))?;
        if let Some(error) = body.get("error") {
            return Err(CallError::Refused(format!("{method}: {error}")));
        }
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    }

    /// `jackdaw/status`, and how long it took.
    pub fn status(&self) -> Result<(Value, f64), CallError> {
        let start = Instant::now();
        let status = self.call("jackdaw/status", json!({}), STALL_TIMEOUT)?;
        Ok((status, ms_since(start)))
    }

    /// Call an operator, and how long the call took. Anything but a finished
    /// operator is an error.
    pub fn operator(&self, id: &str, params: Value) -> Result<(Value, f64), CallError> {
        let start = Instant::now();
        let answer = self.call(
            "jackdaw/call_operator",
            json!({ "id": id, "params": params }),
            STALL_TIMEOUT,
        )?;
        let took = ms_since(start);
        if answer.get("result").and_then(Value::as_str) != Some("finished") {
            return Err(CallError::Refused(format!("{id}: {answer}")));
        }
        Ok((answer, took))
    }
}

pub fn ms_since(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
