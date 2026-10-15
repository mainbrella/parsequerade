use crate::{contract::Route, Client, Error, ErrorDetail, Result};
use reqwest::{header, Method, Url};
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime};

const RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const TOTAL_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct Budget {
    deadline: Instant,
    attempts: usize,
    bytes: usize,
}
impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(25),
            attempts: 0,
            bytes: 0,
        }
    }
}

fn budget_error() -> Error {
    let mut error = Error::new(
        504,
        "upstream_budget_exceeded",
        "The bounded Apple read could not finish.",
    );
    error.retryable = true;
    error
}

fn upstream_error(status: u16, payload: &Value, after: Option<&str>, route: &Route) -> Error {
    let (mapped, code, message) = match status {
        401 => (
            502,
            "upstream_authentication_failed",
            "Apple rejected the configured credentials.",
        ),
        403 => (
            403,
            "upstream_forbidden",
            "The Apple key cannot access this resource.",
        ),
        404 => (
            404,
            "resource_not_found",
            "The requested resource was not found.",
        ),
        409 => (
            409,
            "upstream_conflict",
            "Apple reported a resource conflict.",
        ),
        400 | 422 => (
            422,
            "validation_failed",
            "Apple rejected one or more request parameters.",
        ),
        429 => (429, "rate_limited", "Apple is throttling this connection."),
        _ => (
            if status >= 500 { 503 } else { 502 },
            "upstream_unavailable",
            "Apple could not complete this request.",
        ),
    };
    let mut error = Error::new(mapped, code, message);
    error.retryable = status == 429 || status >= 500;
    error.retry_after = after
        .and_then(|v| {
            v.parse::<u64>().ok().or_else(|| {
                httpdate::parse_http_date(v)
                    .ok()?
                    .duration_since(SystemTime::now())
                    .ok()
                    .map(|d| d.as_secs() + u64::from(d.subsec_nanos() > 0))
            })
        })
        .map(|n| n.min(86400));
    if let Some(errors) = payload["errors"].as_array() {
        error.details = errors
            .iter()
            .take(32)
            .map(|e| {
                let code = e["code"]
                    .as_str()
                    .filter(|v| {
                        !v.is_empty()
                            && v.len() <= 128
                            && v.bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
                    })
                    .unwrap_or("upstream_error");
                ErrorDetail {
                    code: code.into(),
                    message: message.into(),
                    field: route
                        .parameters
                        .iter()
                        .find(|p| {
                            p.apple_name
                                .as_deref()
                                .is_some_and(|n| Some(n) == e["source"]["parameter"].as_str())
                        })
                        .map(|p| p.name.clone()),
                }
            })
            .collect();
    }
    error
}

impl Client {
    pub(crate) async fn apple(
        &self,
        url: Url,
        body: Option<&Value>,
        budget: &mut Budget,
        route: &Route,
    ) -> Result<Value> {
        let write = body.is_some();
        if url.origin() != self.inner.origin.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !url.path().starts_with("/v1/")
            || !url
                .path()
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_-".contains(&c))
        {
            return Err(Error::invalid_upstream());
        }
        let _permit = self.inner.reads.try_acquire().map_err(|_| {
            let mut e = Error::new(
                429,
                "rate_limited",
                "This client has too many concurrent Apple requests.",
            );
            e.retryable = !write;
            e.retry_after = Some(1);
            e
        })?;
        let mut refreshed = false;
        for attempt in 0..if write { 1 } else { 3 } {
            if budget.attempts >= 32 || budget.deadline <= Instant::now() {
                let mut error = budget_error();
                error.retryable = !write;
                return Err(error);
            }
            budget.attempts += 1;
            let jwt = self.inner.auth.token()?;
            let timeout = Duration::from_secs(8)
                .min(budget.deadline.saturating_duration_since(Instant::now()));
            let mut retry_auth = false;
            let operation = async {
                let mut request = self
                    .inner
                    .http
                    .request(if write { Method::PATCH } else { Method::GET }, url.clone())
                    .bearer_auth(&jwt)
                    .header(header::ACCEPT, "application/json");
                if let Some(body) = body {
                    request = request.json(body);
                }
                let mut response = request.send().await.map_err(|_| transport_error(write))?;
                let status = response.status().as_u16();
                if (300..400).contains(&status) {
                    return Err(Error::invalid_upstream());
                }
                if response
                    .content_length()
                    .is_some_and(|n| n > RESPONSE_BYTES as u64)
                {
                    return Err(Error::invalid_upstream());
                }
                let after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                if response.status().is_success() {
                    let mime = response
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    if mime != "application/json"
                        && !(mime.starts_with("application/") && mime.ends_with("+json"))
                    {
                        return Err(Error::invalid_upstream());
                    }
                }
                let mut bytes = Vec::new();
                while let Some(chunk) =
                    response.chunk().await.map_err(|_| transport_error(write))?
                {
                    budget.bytes += chunk.len();
                    if bytes.len() + chunk.len() > RESPONSE_BYTES || budget.bytes > TOTAL_BYTES {
                        return Err(Error::invalid_upstream());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if (200..300).contains(&status) {
                    if !payload.is_object() || payload.get("data").is_none() {
                        return Err(Error::invalid_upstream());
                    }
                    Ok(payload)
                } else {
                    let refresh = status == 401 && !write && !refreshed;
                    if refresh {
                        self.inner.auth.invalidate(&jwt);
                        refreshed = true;
                        retry_auth = true;
                    }
                    let mut error = upstream_error(status, &payload, after.as_deref(), route);
                    if write {
                        error.retryable = false;
                    }
                    Err(error)
                }
            };
            let outcome = tokio::time::timeout(timeout, operation)
                .await
                .unwrap_or_else(|_| {
                    let mut e = transport_error(write);
                    if !write {
                        e.status = 504;
                        e.code = "upstream_timeout".into();
                    }
                    Err(e)
                });
            match outcome {
                Ok(value) => return Ok(value),
                Err(mut error) => {
                    if write {
                        if error.code == "upstream_invalid_response" {
                            error = Error::unknown_write();
                        }
                        return Err(error);
                    }
                    if (!error.retryable && !retry_auth) || attempt == 2 {
                        return Err(error);
                    }
                    let delay = if retry_auth {
                        Duration::ZERO
                    } else {
                        Duration::from_millis(
                            error
                                .retry_after
                                .map(|v| v * 1000)
                                .unwrap_or(200 << attempt),
                        )
                    };
                    if delay > Duration::from_secs(2) || Instant::now() + delay >= budget.deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
        Err(budget_error())
    }
}

fn transport_error(write: bool) -> Error {
    if write {
        Error::unknown_write()
    } else {
        let mut error = Error::new(
            503,
            "upstream_unavailable",
            "The Apple read could not complete.",
        );
        error.retryable = true;
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        contract,
        tests::{mock, Step},
    };
    use serde_json::json;

    #[tokio::test]
    async fn exhausted_budgets_prevent_dispatch() {
        let (client, task) = mock(vec![]).await;
        for (attempts, deadline) in [
            (32, Instant::now() + Duration::from_secs(25)),
            (0, Instant::now()),
        ] {
            for body in [None, Some(json!({"data":{}}))] {
                let mut budget = Budget {
                    deadline,
                    attempts,
                    bytes: 0,
                };
                let error = client
                    .apple(
                        client.inner.origin.join("/v1/apps/app1").unwrap(),
                        body.as_ref(),
                        &mut budget,
                        contract::route("app"),
                    )
                    .await
                    .unwrap_err();
                assert_eq!(error.code, "upstream_budget_exceeded");
                assert_eq!(error.retryable, body.is_none());
            }
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn deadlines_cover_the_entire_response_and_writes_stay_uncertain() {
        for write in [false, true] {
            let body = json!({"data":{}});
            let mut step = Step::get(
                "/v1/apps/app1",
                json!({"data":{"id":"app1", "type":"apps"}}),
            )
            .delay(Duration::from_millis(80));
            if write {
                step = step.patch(body.clone());
            }
            let (client, task) = mock(vec![step]).await;
            let mut budget = Budget {
                deadline: Instant::now() + Duration::from_millis(30),
                attempts: 0,
                bytes: 0,
            };
            let error = client
                .apple(
                    client.inner.origin.join("/v1/apps/app1").unwrap(),
                    if write { Some(&body) } else { None },
                    &mut budget,
                    contract::route("app"),
                )
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                if write {
                    "write_outcome_unknown"
                } else {
                    "upstream_timeout"
                }
            );
            assert_eq!(error.retryable, !write);
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn response_and_total_body_limits_are_enforced() {
        for (payload, used) in [
            (json!({"data":{"text":"x".repeat(RESPONSE_BYTES)}}), 0),
            (json!({"data":{}}), TOTAL_BYTES),
        ] {
            let (client, task) = mock(vec![Step::get("/v1/apps/app1", payload)]).await;
            let mut budget = Budget {
                bytes: used,
                ..Budget::new()
            };
            let error = client
                .apple(
                    client.inner.origin.join("/v1/apps/app1").unwrap(),
                    None,
                    &mut budget,
                    contract::route("app"),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "upstream_invalid_response");
            task.await.unwrap();
        }
    }
}
