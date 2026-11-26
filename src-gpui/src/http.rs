use std::sync::OnceLock;

use postlite_core::RequestPayload;
use serde_json::Value;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

pub struct Response {
    pub wire: postlite_core::ResponsePayload,
    pub pretty: Option<String>,
}

pub fn send(payload: Value) -> Result<Response, String> {
    let request: RequestPayload =
        serde_json::from_value(payload).map_err(|error| error.to_string())?;
    let runtime = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let wire = runtime.block_on(postlite_core::send(request));
    if let Some(error) = wire.error.as_deref() {
        return Err(error.to_owned());
    }
    let pretty = pretty(&wire.body, wire.truncated);
    Ok(Response { wire, pretty })
}

pub fn pretty(body: &str, truncated: bool) -> Option<String> {
    if !truncated && body.len() <= 256 * 1024 {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .filter(|pretty| pretty.len() <= 2_000_000)
    } else {
        None
    }
}
