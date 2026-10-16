//! Host HTTP boundary for language bindings and trusted custom transports.
//!
//! The core owns authentication, validation, retries and operation deadlines. A transport
//! sends exactly one request and must not retry it, follow redirects, or log credentials.
use crate::Client;
use std::{collections::BTreeMap, future::Future, pin::Pin, time::Duration};

/// One fully prepared request. Blob uploads never include the Apple bearer token.
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

/// A bounded response. Header names are matched without regard to case.
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Transport failures deliberately carry no raw URLs, credentials or host error text.
#[derive(Debug, Clone, Copy)]
pub enum HttpError {
    Timeout,
    Failed,
    InvalidResponse,
}

/// A trusted adapter used by language bindings. Implementations must honor the timeout,
/// bound response reading, and return without dispatching any additional requests.
pub trait HttpTransport: Send + Sync {
    fn send(
        &self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<HttpResponse, HttpError>> + Send + '_>>;
}

impl Client {
    pub(crate) async fn send_http(
        &self,
        request: HttpRequest,
    ) -> std::result::Result<HttpResponse, HttpError> {
        self.send_http_counted(request, &mut 0).await
    }

    pub(crate) async fn send_http_counted(
        &self,
        request: HttpRequest,
        bytes_read: &mut usize,
    ) -> std::result::Result<HttpResponse, HttpError> {
        let timeout = request.timeout;
        let max_bytes = request.max_response_bytes;
        let operation = async {
            if let Some(transport) = &self.inner.transport {
                let response = transport.send(request).await?;
                *bytes_read += response.body.len();
                if !(100..=599).contains(&response.status) || response.body.len() > max_bytes {
                    return Err(HttpError::InvalidResponse);
                }
                return Ok(response);
            }
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|_| HttpError::Failed)?;
            let mut builder = self.inner.http.request(method, &request.url);
            for (name, value) in request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let mut response = builder.send().await.map_err(|_| HttpError::Failed)?;
            if response
                .content_length()
                .is_some_and(|n| n > max_bytes as u64)
            {
                return Err(HttpError::InvalidResponse);
            }
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.to_string(), value.to_str().ok()?.to_owned()))
                })
                .collect();
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| HttpError::Failed)? {
                // Charge partial bodies even when reading fails or the future times out.
                *bytes_read += chunk.len();
                if body.len() + chunk.len() > max_bytes {
                    return Err(HttpError::InvalidResponse);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        };
        tokio::time::timeout(timeout, operation)
            .await
            .unwrap_or(Err(HttpError::Timeout))
    }
}
