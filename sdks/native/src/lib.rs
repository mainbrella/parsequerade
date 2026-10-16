//! A host-neutral bridge. Every operation executes in the canonical Rust SDK.
//!
//! Hosts service HTTP/checkpoint events and acknowledge them before execution continues.
//! This avoids foreign callbacks, borrowed host pointers and blocking async host runtimes.
use base64::{engine::general_purpose::STANDARD, Engine};
use parsequerade::{
    Client, Error, HttpError, HttpRequest, HttpResponse, HttpTransport, ListingImageUpload,
    ListingImageUploadCheckpoint, Query,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::{c_char, CStr, CString},
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{oneshot, Notify};

tokio::task_local! { static ACTIVE: Arc<Operation>; }

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("native runtime")
    })
}

fn clients() -> &'static Mutex<HashMap<u64, Client>> {
    static CLIENTS: OnceLock<Mutex<HashMap<u64, Client>>> = OnceLock::new();
    CLIENTS.get_or_init(Default::default)
}

fn operations() -> &'static Mutex<HashMap<u64, Arc<Operation>>> {
    static OPERATIONS: OnceLock<Mutex<HashMap<u64, Arc<Operation>>>> = OnceLock::new();
    OPERATIONS.get_or_init(Default::default)
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn error(status: u16, code: &str, message: &str) -> Error {
    Error {
        status,
        code: code.into(),
        message: message.into(),
        retryable: false,
        details: vec![],
        retry_after: None,
    }
}

fn invalid() -> Error {
    error(400, "invalid_request", "The SDK request is invalid.")
}
fn cancelled(write: bool) -> Error {
    if write {
        error(503, "write_outcome_unknown", "The operation was cancelled. Read the affected resource or resume with the same checkpoint before trying again.")
    } else {
        error(499, "operation_cancelled", "The operation was cancelled.")
    }
}

fn encoded_error(error: Error) -> String {
    json!({"error":error}).to_string()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    issuer_id: String,
    key_id: String,
    private_key: String,
    #[serde(default)]
    custom_transport: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    #[serde(default)]
    args: Vec<Value>,
    png: Option<String>,
}

struct Operation {
    events: mpsc::Sender<Value>,
    receiver: Mutex<mpsc::Receiver<Value>>,
    responses: Mutex<HashMap<u64, PendingResponse>>,
    checkpoint: Mutex<Option<Value>>,
    cancelled: AtomicBool,
    notify: Notify,
    write: bool,
}

struct PendingResponse {
    sender: oneshot::Sender<Value>,
    deadline: Option<Instant>,
}

impl Operation {
    fn request(&self, event: Value) -> Result<oneshot::Receiver<Value>, Error> {
        self.request_until(event, None)
    }

    fn request_until(
        &self,
        mut event: Value,
        deadline: Option<Instant>,
    ) -> Result<oneshot::Receiver<Value>, Error> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(cancelled(self.write));
        }
        let id = next_id();
        let (send, receive) = oneshot::channel();
        let mut responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
        // Coordinate with cancel so a persistence wait cannot outlive cancellation.
        if self.cancelled.load(Ordering::Acquire) {
            return Err(cancelled(self.write));
        }
        let deadline = deadline.or_else(|| {
            event["timeoutMs"]
                .as_u64()
                .map(|milliseconds| Instant::now() + Duration::from_millis(milliseconds))
        });
        if let Some(deadline) = deadline {
            let milliseconds = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64;
            event["deadlineMs"] = (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64)
                .saturating_add(milliseconds)
                .into();
            event["timeoutMs"] = milliseconds.into();
        }
        responses.insert(
            id,
            PendingResponse {
                sender: send,
                deadline,
            },
        );
        event["id"] = id.into();
        self.events.send(event).map_err(|_| cancelled(self.write))?;
        Ok(receive)
    }

    fn persist(&self, checkpoint: &ListingImageUploadCheckpoint) -> Result<(), Error> {
        let checkpoint = serde_json::to_value(checkpoint).map_err(|_| invalid())?;
        *self.checkpoint.lock().unwrap_or_else(|e| e.into_inner()) = Some(checkpoint.clone());
        let response = self.request(json!({"kind":"checkpoint", "checkpoint":checkpoint}))?;
        let response = tokio::task::block_in_place(|| response.blocking_recv())
            .map_err(|_| cancelled(self.write))?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(cancelled(self.write));
        }
        if response["ok"] == true {
            Ok(())
        } else {
            Err(error(
                500,
                "checkpoint_persistence_failed",
                "Could not persist the upload checkpoint.",
            ))
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.notify.notify_one();
    }
}

struct EventTransport;
impl HttpTransport for EventTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + '_>> {
        let operation = ACTIVE.try_with(Arc::clone).ok();
        Box::pin(async move {
            let operation = operation.ok_or(HttpError::Failed)?;
            // Encoding a large upload chunk consumes the existing attempt budget.
            let deadline = Instant::now() + request.timeout;
            let response = operation.request_until(json!({
                "kind":"http", "method":request.method, "url":request.url,
                "headers":request.headers, "body":request.body.map(|body| STANDARD.encode(body)),
                "timeoutMs":request.timeout.as_millis().max(1) as u64,
                "maxResponseBytes":request.max_response_bytes,
            }), Some(deadline)).map_err(|_| HttpError::Failed)?.await.map_err(|_| HttpError::Failed)?;
            if let Some(error) = response.get("error") {
                return Err(match error.as_str() {
                    Some("timeout") => HttpError::Timeout,
                    Some("failed") => HttpError::Failed,
                    _ => HttpError::InvalidResponse,
                });
            }
            let status = response["status"]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or(HttpError::InvalidResponse)?;
            let headers = serde_json::from_value(response["headers"].clone())
                .map_err(|_| HttpError::InvalidResponse)?;
            let body = STANDARD
                .decode(
                    response["body"]
                        .as_str()
                        .ok_or(HttpError::InvalidResponse)?,
                )
                .map_err(|_| HttpError::InvalidResponse)?;
            if body.len() > request.max_response_bytes {
                return Err(HttpError::InvalidResponse);
            }
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }
}

pub fn abi_version() -> u32 {
    1
}

pub fn client_new_json(config: &str) -> String {
    let config: Config = match serde_json::from_str(config) {
        Ok(config) => config,
        Err(_) => {
            return encoded_error(error(
                400,
                "invalid_credentials",
                "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.",
            ))
        }
    };
    let client = if config.custom_transport {
        Client::with_transport(
            &config.issuer_id,
            &config.key_id,
            &config.private_key,
            Arc::new(EventTransport),
        )
    } else {
        Client::new(&config.issuer_id, &config.key_id, &config.private_key)
    };
    match client {
        Ok(client) => {
            let id = next_id();
            clients()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, client);
            json!({"client":id}).to_string()
        }
        Err(error) => encoded_error(error),
    }
}

pub fn client_free(client: u64) {
    clients()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&client);
}

fn validate_request(request: &Request) -> Result<(), Error> {
    let expected = match request.operation.as_str() {
        "get" | "update_metadata" | "upload_listing_image" => 2,
        "update_age_rating" | "create_version" | "attach_build" => 3,
        "listing_image_reference_data" => 0,
        _ => return Err(invalid()),
    };
    if request.args.len() != expected {
        return Err(invalid());
    }
    Ok(())
}

pub fn start_json(client: u64, request: &str) -> String {
    let request: Request = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(_) => return encoded_error(invalid()),
    };
    if let Err(error) = validate_request(&request) {
        return encoded_error(error);
    }
    let client = match clients()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&client)
        .cloned()
    {
        Some(client) => client,
        None => return encoded_error(error(400, "client_closed", "The SDK client is closed.")),
    };
    let write = !matches!(
        request.operation.as_str(),
        "get" | "listing_image_reference_data"
    );
    let (events, receiver) = mpsc::channel();
    let operation = Arc::new(Operation {
        events,
        receiver: Mutex::new(receiver),
        responses: Mutex::new(HashMap::new()),
        checkpoint: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        notify: Notify::new(),
        write,
    });
    let id = next_id();
    operations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, operation.clone());
    let task_operation = operation.clone();
    let task = runtime().spawn(ACTIVE.scope(operation.clone(), async move {
        tokio::select! {
            biased;
            _ = task_operation.notify.notified() => Err(cancelled(write)),
            result = execute(&client, request, &task_operation) => result,
        }
    }));
    runtime().spawn(async move {
        let result = task.await.unwrap_or_else(|_| {
            Err(error(
                500,
                "internal_error",
                "The SDK operation could not complete.",
            ))
        });
        let result = if operation.cancelled.load(Ordering::Acquire) {
            Err(cancelled(write))
        } else {
            result
        };
        let mut event = match result {
            Ok(value) => json!({"kind":"result","value":value}),
            Err(error) => json!({"kind":"result","error":error}),
        };
        if let Some(checkpoint) = operation
            .checkpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            event["checkpoint"] = checkpoint;
        }
        operation
            .responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let _ = operation.events.send(event);
    });
    json!({"operation":id}).to_string()
}

async fn execute(client: &Client, request: Request, operation: &Operation) -> Result<Value, Error> {
    let args = &request.args;
    let arg = |index: usize| args[index].as_str().ok_or_else(invalid);
    match request.operation.as_str() {
        "get" => {
            let pairs: Vec<(String, String)> =
                serde_json::from_value(args[1].clone()).map_err(|_| invalid())?;
            let query = pairs
                .into_iter()
                .fold(Query::new(), |query, (key, value)| query.with(key, value));
            client.get::<Value>(arg(0)?, &query).await
        }
        "update_age_rating" => {
            client
                .update_age_rating_json(arg(0)?, arg(1)?, &args[2])
                .await
        }
        "update_metadata" => client.update_metadata(arg(0)?, &args[1]).await,
        "create_version" => client.create_version(arg(0)?, arg(1)?, arg(2)?).await,
        "attach_build" => client.attach_build(arg(0)?, arg(1)?, arg(2)?).await,
        "listing_image_reference_data" => client.listing_image_reference_data().await,
        "upload_listing_image" => {
            let request_model: ListingImageUpload =
                serde_json::from_value(args[0].clone()).map_err(|_| invalid())?;
            let mut checkpoint: ListingImageUploadCheckpoint =
                serde_json::from_value(args[1].clone()).map_err(|_| invalid())?;
            *operation
                .checkpoint
                .lock()
                .unwrap_or_else(|e| e.into_inner()) =
                Some(serde_json::to_value(&checkpoint).map_err(|_| invalid())?);
            let png = STANDARD
                .decode(request.png.ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            let result = client
                .upload_listing_image(&request_model, &png, &mut checkpoint, |checkpoint| {
                    operation.persist(checkpoint)
                })
                .await;
            *operation
                .checkpoint
                .lock()
                .unwrap_or_else(|e| e.into_inner()) =
                Some(serde_json::to_value(checkpoint).map_err(|_| invalid())?);
            result.and_then(|receipt| serde_json::to_value(receipt).map_err(|_| invalid()))
        }
        _ => Err(invalid()),
    }
}

pub fn poll_json(id: u64, timeout_ms: u32) -> String {
    let operation = operations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned();
    let Some(operation) = operation else {
        return json!({"kind":"result","error":error(400,"operation_closed","The SDK operation is closed.")}).to_string();
    };
    let until = Instant::now() + Duration::from_millis(u64::from(timeout_ms.min(1000)));
    loop {
        let next = operation
            .receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recv_timeout(until.saturating_duration_since(Instant::now()));
        let Ok(mut event) = next else {
            return "null".into();
        };
        if event["kind"] == "result" {
            return event.to_string();
        }
        let Some(event_id) = event["id"].as_u64() else {
            continue;
        };
        let mut responses = operation
            .responses
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(pending) = responses.get(&event_id) else {
            continue;
        };
        if pending.sender.is_closed() || operation.cancelled.load(Ordering::Acquire) {
            responses.remove(&event_id);
            continue;
        }
        if let Some(deadline) = pending.deadline {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64;
            if remaining == 0 {
                if let Some(pending) = responses.remove(&event_id) {
                    let _ = pending.sender.send(json!({"error":"timeout"}));
                }
                continue;
            }
            event["timeoutMs"] = remaining.into();
        }
        return event.to_string();
    }
}

pub fn respond_json(id: u64, event: u64, response: &str) -> bool {
    let operation = operations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned();
    let response = match serde_json::from_str(response) {
        Ok(response) => response,
        Err(_) => return false,
    };
    operation
        .and_then(|operation| {
            operation
                .responses
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&event)
        })
        .is_some_and(|pending| {
            if pending
                .deadline
                .is_some_and(|deadline| deadline <= Instant::now())
            {
                let _ = pending.sender.send(json!({"error":"timeout"}));
                false
            } else {
                pending.sender.send(response).is_ok()
            }
        })
}

pub fn cancel(id: u64) {
    if let Some(operation) = operations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
    {
        operation.cancel();
    }
}
pub fn operation_free(id: u64) {
    if let Some(operation) = operations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
    {
        operation.cancel();
    }
}

fn c_result(action: impl FnOnce() -> String) -> *mut c_char {
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_or_else(|_| {
            encoded_error(error(
                500,
                "internal_error",
                "The SDK operation could not complete.",
            ))
        });
    CString::new(result)
        .expect("JSON has no NUL bytes")
        .into_raw()
}

unsafe fn input<'a>(pointer: *const c_char) -> Option<&'a str> {
    if pointer.is_null() {
        None
    } else {
        CStr::from_ptr(pointer).to_str().ok()
    }
}

#[no_mangle]
pub extern "C" fn pq_abi_version() -> u32 {
    abi_version()
}
/// # Safety
/// `config` must point to a NUL-terminated UTF-8 string for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn pq_client_new(config: *const c_char) -> *mut c_char {
    c_result(|| client_new_json(input(config).unwrap_or("")))
}
#[no_mangle]
pub extern "C" fn pq_client_free(id: u64) {
    client_free(id)
}
/// # Safety
/// `request` must point to a NUL-terminated UTF-8 string for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn pq_start(id: u64, request: *const c_char) -> *mut c_char {
    c_result(|| start_json(id, input(request).unwrap_or("")))
}
#[no_mangle]
pub extern "C" fn pq_poll(id: u64, timeout_ms: u32) -> *mut c_char {
    c_result(|| poll_json(id, timeout_ms))
}
/// # Safety
/// `response` must point to a NUL-terminated UTF-8 string for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn pq_respond(id: u64, event: u64, response: *const c_char) -> bool {
    input(response).is_some_and(|response| respond_json(id, event, response))
}
#[no_mangle]
pub extern "C" fn pq_cancel(id: u64) {
    cancel(id)
}
#[no_mangle]
pub extern "C" fn pq_operation_free(id: u64) {
    operation_free(id)
}
/// # Safety
/// Pass only a non-freed pointer returned by this library, or NULL.
#[no_mangle]
pub unsafe extern "C" fn pq_string_free(pointer: *mut c_char) {
    if !pointer.is_null() {
        drop(CString::from_raw(pointer));
    }
}

#[cfg(test)]
mod tests;
