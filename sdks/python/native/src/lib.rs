//! Language bindings only: validation, HTTP policy and workflows belong to parsequerade.
use parsequerade::{
    Client, Error, HttpError, HttpRequest, HttpResponse, HttpTransport, ListingImageUpload,
    ListingImageUploadCheckpoint, Query,
};
use pyo3::{
    class::gc::{PyTraverseError, PyVisit},
    create_exception,
    exceptions::{PyException, PyRuntimeError, PyTimeoutError},
    prelude::*,
    types::{PyBool, PyBytes, PyDict},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
};

create_exception!(_native, CoreError, PyException);

fn runtime() -> PyResult<&'static tokio::runtime::Runtime> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(PyRuntimeError::new_err(
            "SDK calls cannot run inside a transport or persistence callback.",
        ));
    }
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, ()>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| PyRuntimeError::new_err("Could not initialize the SDK runtime."))
}

fn local_error(code: &str, message: &str) -> Error {
    Error {
        status: 400,
        code: code.into(),
        message: message.into(),
        retryable: false,
        details: vec![],
        retry_after: None,
    }
}

fn exception(error: Error) -> PyErr {
    CoreError::new_err(serde_json::to_string(&error).expect("serializable core error"))
}

fn parse<T: serde::de::DeserializeOwned>(encoded: &str) -> PyResult<T> {
    serde_json::from_str(encoded).map_err(|_| {
        exception(local_error(
            "invalid_request",
            "The request or checkpoint is invalid.",
        ))
    })
}

fn envelope<T: Serialize>(result: parsequerade::Result<T>) -> String {
    let value = match result {
        Ok(value) => json!({"value": value}),
        Err(error) => json!({"error": error}),
    };
    serde_json::to_string(&value).expect("serializable core response")
}

struct PythonTransport {
    callback: Mutex<Option<Py<PyAny>>>,
}

impl HttpTransport for PythonTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + '_>> {
        let callback = Python::attach(|py| {
            self.callback
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|callback| callback.clone_ref(py))
        });
        Box::pin(async move {
            let callback = callback.ok_or(HttpError::Failed)?;
            tokio::task::spawn_blocking(move || {
                Python::attach(|py| {
                    let response = (|| -> PyResult<HttpResponse> {
                        let headers = PyDict::new(py);
                        for (key, value) in request.headers {
                            headers.set_item(key, value)?;
                        }
                        let body = request.body.as_deref().map(|bytes| PyBytes::new(py, bytes));
                        let response = callback.call1(
                            py,
                            (
                                request.method,
                                request.url,
                                headers,
                                body,
                                request.timeout.as_secs_f64(),
                            ),
                        )?;
                        let response = response.bind(py);
                        let status = response.getattr("status")?;
                        if status.is_instance_of::<PyBool>() {
                            return Err(pyo3::exceptions::PyTypeError::new_err(
                                "Invalid response status",
                            ));
                        }
                        let status = status.extract::<u16>()?;
                        let headers = response
                            .getattr("headers")?
                            .extract::<BTreeMap<String, String>>()?
                            .into_iter()
                            .map(|(key, value)| (key.to_ascii_lowercase(), value))
                            .collect();
                        let body = response.getattr("body")?;
                        let body = body.cast::<PyBytes>()?.as_bytes();
                        if body.len() > request.max_response_bytes {
                            return Err(pyo3::exceptions::PyValueError::new_err(
                                "Response exceeds its bound",
                            ));
                        }
                        Ok(HttpResponse {
                            status,
                            headers,
                            body: body.to_vec(),
                        })
                    })();
                    response.map_err(|error| {
                        if error.is_instance_of::<PyTimeoutError>(py) {
                            HttpError::Timeout
                        } else if error.is_instance_of::<pyo3::exceptions::PyOSError>(py) {
                            HttpError::Failed
                        } else {
                            HttpError::InvalidResponse
                        }
                    })
                })
            })
            .await
            .map_err(|_| HttpError::Failed)?
        })
    }
}

#[pyclass(name = "Client", frozen)]
struct NativeClient {
    client: Client,
    transport: Option<Arc<PythonTransport>>,
}

#[pymethods]
impl NativeClient {
    #[new]
    #[pyo3(signature = (issuer_id, key_id, private_key, transport=None))]
    fn new(
        py: Python<'_>,
        issuer_id: &str,
        key_id: &str,
        private_key: &str,
        transport: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        runtime()?;
        let transport = if let Some(callback) = transport {
            if !callback.bind(py).is_callable() {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "transport must be callable",
                ));
            }
            Some(Arc::new(PythonTransport {
                callback: Mutex::new(Some(callback)),
            }))
        } else {
            None
        };
        let client = if let Some(transport) = &transport {
            Client::with_transport(issuer_id, key_id, private_key, transport.clone())
        } else {
            Client::new(issuer_id, key_id, private_key)
        };
        Ok(Self {
            client: client.map_err(exception)?,
            transport,
        })
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Some(transport) = &self.transport {
            let callback = transport
                .callback
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(callback) = callback.as_ref() {
                visit.call(callback)?;
            }
        }
        Ok(())
    }

    fn __clear__(&self) {
        if let Some(transport) = &self.transport {
            let callback = {
                let mut callback = transport
                    .callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                callback.take()
            };
            // Dropping a callable can execute Python finalizers; release the lock first.
            drop(callback);
        }
    }

    fn get(&self, py: Python<'_>, path: String, query: Vec<(String, String)>) -> PyResult<String> {
        let runtime = runtime()?;
        let query = query
            .into_iter()
            .fold(Query::new(), |query, (name, value)| query.with(name, value));
        Ok(py.detach(|| envelope(runtime.block_on(self.client.get::<Value>(&path, &query)))))
    }

    fn update_age_rating(
        &self,
        py: Python<'_>,
        app_id: String,
        app_info_id: String,
        attributes: &str,
    ) -> PyResult<String> {
        let attributes: Value = parse(attributes)?;
        let runtime = runtime()?;
        Ok(py.detach(|| {
            envelope(runtime.block_on(self.client.update_age_rating_json(
                &app_id,
                &app_info_id,
                &attributes,
            )))
        }))
    }

    fn update_metadata(&self, py: Python<'_>, path: String, attributes: &str) -> PyResult<String> {
        let attributes: Value = parse(attributes)?;
        let runtime = runtime()?;
        Ok(py
            .detach(|| envelope(runtime.block_on(self.client.update_metadata(&path, &attributes)))))
    }

    fn create_version(
        &self,
        py: Python<'_>,
        app_id: String,
        platform: String,
        version_string: String,
    ) -> PyResult<String> {
        let runtime = runtime()?;
        Ok(py.detach(|| {
            envelope(
                runtime.block_on(
                    self.client
                        .create_version(&app_id, &platform, &version_string),
                ),
            )
        }))
    }

    fn attach_build(
        &self,
        py: Python<'_>,
        app_id: String,
        version_id: String,
        build_id: String,
    ) -> PyResult<String> {
        let runtime = runtime()?;
        Ok(py.detach(|| {
            envelope(runtime.block_on(self.client.attach_build(&app_id, &version_id, &build_id)))
        }))
    }

    fn listing_image_reference_data(&self, py: Python<'_>) -> PyResult<String> {
        let runtime = runtime()?;
        Ok(py.detach(|| envelope(runtime.block_on(self.client.listing_image_reference_data()))))
    }

    fn upload_listing_image(
        &self,
        py: Python<'_>,
        request: &str,
        png: Vec<u8>,
        checkpoint: &str,
        persist: Py<PyAny>,
    ) -> PyResult<(String, Option<Py<PyAny>>)> {
        let request: ListingImageUpload = parse(request)?;
        let mut checkpoint: ListingImageUploadCheckpoint = parse(checkpoint)?;
        let runtime = runtime()?;
        let (result, checkpoint, callback_error) = py.detach(|| {
            let mut callback_error = None;
            let result = runtime.block_on(self.client.upload_listing_image(
                &request,
                &png,
                &mut checkpoint,
                |snapshot| {
                    let encoded = serde_json::to_string(snapshot).expect("serializable checkpoint");
                    Python::attach(|py| persist.call1(py, (encoded,)))
                        .map(|_| ())
                        .map_err(|error| {
                            callback_error = Some(error);
                            local_error(
                                "checkpoint_persistence_failed",
                                "Could not persist the upload checkpoint.",
                            )
                        })
                },
            ));
            (result, checkpoint, callback_error)
        });
        let mut response: Value = serde_json::from_str(&envelope(result)).expect("valid envelope");
        response["checkpoint"] = serde_json::to_value(checkpoint).expect("serializable checkpoint");
        let callback_error = callback_error.map(|error| error.into_value(py).into_any());
        Ok((
            serde_json::to_string(&response).expect("serializable upload response"),
            callback_error,
        ))
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<NativeClient>()?;
    module.add("Error", module.py().get_type::<CoreError>())?;
    Ok(())
}
