use anyhow::anyhow;
use bevy::{remote::BrpRequest, tasks::IoTaskPool};

/// Send a BRP JSON-RPC 2.0 request and return the result as a background task.
///
/// Follows the same pattern as `src/navmesh/brp_client.rs`. Uses `ehttp::fetch_async`
/// on the `IoTaskPool`.
pub fn brp_request(
    endpoint: &str,
    method: &str,
    params: Option<serde_json::Value>,
) -> bevy::tasks::Task<Result<serde_json::Value, anyhow::Error>> {
    let req = BrpRequest {
        jsonrpc: String::from("2.0"),
        method: String::from(method),
        id: None,
        params,
    };

    let url = endpoint.to_string();

    let future = async move {
        let request = ehttp::Request::json(&url, &req)?;
        let resp = ehttp::fetch_async(request)
            .await
            .map_err(|s| anyhow!("{s}"))?;

        let mut v: serde_json::Value = resp.json()?;

        if let Some(val) = v.get_mut("result") {
            Ok(val.take())
        } else if let Some(error) = v.get("error") {
            Err(anyhow!("BRP error: {error}"))
        } else {
            Err(anyhow!(
                "BRP error: Response returned neither 'result' nor 'error' field"
            ))
        }
    };

    IoTaskPool::get().spawn(future)
}
