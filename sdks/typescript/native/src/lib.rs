//! Thin Node bindings: the language-neutral bridge owns all SDK behavior.
use napi_derive::napi;

#[napi]
pub fn abi_version() -> u32 {
    parsequerade_native::abi_version()
}

#[napi]
pub fn client_new_json(config: String) -> String {
    parsequerade_native::client_new_json(&config)
}

#[napi]
pub fn client_free(client: f64) {
    parsequerade_native::client_free(client as u64);
}

#[napi]
pub fn start_json(client: f64, request: String) -> String {
    parsequerade_native::start_json(client as u64, &request)
}

#[napi]
pub async fn poll_json(operation: f64, timeout_ms: u32) -> napi::Result<String> {
    napi::tokio::task::spawn_blocking(move || {
        parsequerade_native::poll_json(operation as u64, timeout_ms)
    })
    .await
    .map_err(|_| napi::Error::from_reason("The SDK operation could not be polled."))
}

#[napi]
pub fn respond_json(operation: f64, event: f64, response: String) -> bool {
    parsequerade_native::respond_json(operation as u64, event as u64, &response)
}

#[napi]
pub fn cancel(operation: f64) {
    parsequerade_native::cancel(operation as u64);
}

#[napi]
pub fn operation_free(operation: f64) {
    parsequerade_native::operation_free(operation as u64);
}
