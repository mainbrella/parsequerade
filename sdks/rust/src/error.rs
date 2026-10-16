use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

/// Stable facade error. Raw Apple messages, request URLs and credentials are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Vec<ErrorDetail>,
    pub retry_after: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl Error {
    pub(crate) fn new(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: Vec::new(),
            retry_after: None,
        }
    }

    pub(crate) fn invalid_query(field: &str) -> Self {
        let mut error = Self::new(400, "invalid_query", "The request query is invalid.");
        error.details.push(ErrorDetail {
            code: "invalid_parameter".into(),
            message: "Provide a supported parameter and value.".into(),
            field: Some(field.into()),
        });
        error
    }

    pub(crate) fn invalid_upstream() -> Self {
        Self::new(
            502,
            "upstream_invalid_response",
            "Apple returned an invalid response.",
        )
    }

    pub(crate) fn not_found(code: &str) -> Self {
        Self::new(404, code, "The requested resource was not found.")
    }

    pub(crate) fn incomplete() -> Self {
        Self::new(
            502,
            "upstream_incomplete_result",
            "Apple pagination exceeded its bound or repeated a page.",
        )
    }

    pub(crate) fn unknown_write() -> Self {
        Self::new(
            503,
            "write_outcome_unknown",
            "Apple did not confirm this write. Read the affected resource before trying again.",
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}
