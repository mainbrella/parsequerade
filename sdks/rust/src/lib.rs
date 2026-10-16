#![doc = include_str!("../README.md")]

mod auth;
mod contract;
mod error;
mod generated;
mod pagination;
mod resources;
mod transport;
mod upload;
mod versions;

pub use error::{Error, ErrorDetail, Result};
pub use generated::*;
use reqwest::Url;
use ring::{hmac, rand::SystemRandom};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
pub use upload::{
    ListingImagePlacementType, ListingImageUpload, ListingImageUploadCheckpoint,
    ListingImageUploadReceipt,
};

/// Facade query names, e.g. `bundleId`, `platform`, `state`, `fields`, `limit`, `cursor`.
/// Values use the hosted facade's comma-separated format and are validated per route.
#[derive(Debug, Clone, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a query parameter. Unknown names, invalid values and duplicates fail locally.
    pub fn with(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.0.push((name.into(), value.to_string()));
        self
    }
}

/// A bounded page with an opaque cursor tied to this client and the original query.
/// Cursors last 15 minutes and can be used by clones of the same client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

struct Inner {
    http: reqwest::Client,
    auth: auth::Auth,
    origin: Url,
    cursor_key: hmac::Key,
    reads: tokio::sync::Semaphore,
}

/// A local facade backed by an Apple team API key. No hosted connection or facade key is needed.
/// Apple permissions apply directly; there is no additional hosted grant layer.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Load an Apple team key. `private_key_pem` is the `.p8` contents, not a file path.
    /// Validates the key locally and makes no network requests.
    pub fn new(issuer_id: &str, key_id: &str, private_key_pem: &str) -> Result<Self> {
        let auth = auth::Auth::new(issuer_id, key_id, private_key_pem)?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(8))
            .user_agent(concat!("parsequerade-rust/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| {
                Error::new(
                    500,
                    "client_configuration_failed",
                    "Could not initialize the HTTP client.",
                )
            })?;
        let cursor_key =
            hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new()).map_err(|_| {
                Error::new(
                    500,
                    "client_configuration_failed",
                    "Could not initialize cursor signing.",
                )
            })?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                auth,
                origin: Url::parse("https://api.appstoreconnect.apple.com").expect("Apple origin"),
                cursor_key,
                reads: tokio::sync::Semaphore::new(4),
            }),
        })
    }

    /// Read an implemented facade path, relative to a connection (`/apps/...`).
    /// Use `serde_json::Value` as `T` to preserve the exact flat JSON, including nulls.
    /// Convenience methods such as [`Client::apps`] return typed resources.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, query: &Query) -> Result<T> {
        let value = self
            .read(path, query, &mut transport::Budget::new())
            .await?;
        serde_json::from_value(value).map_err(|_| {
            Error::new(
                500,
                "response_type_mismatch",
                "The requested Rust type does not match the facade response.",
            )
        })
    }

    /// Update only the supplied flat age-rating attributes. Performs the same scoped
    /// parent/declaration resolution as the Worker and sends exactly one Apple PATCH.
    /// Writes are never retried. On `write_outcome_unknown`, read back before deciding to retry.
    pub async fn update_age_rating(
        &self,
        app_id: &str,
        app_info_id: &str,
        attributes: &Value,
    ) -> Result<AgeRating> {
        contract::validate_id(app_id)?;
        contract::validate_id(app_info_id)?;
        contract::validate_update(attributes)?;
        let mut budget = transport::Budget::new();
        let current = self
            .read(
                &format!("/apps/{app_id}/app-infos/{app_info_id}/age-rating"),
                &Query::new(),
                &mut budget,
            )
            .await?;
        let id = current["id"].as_str().ok_or_else(Error::invalid_upstream)?;
        let route = contract::route("ageRating");
        let url = self
            .inner
            .origin
            .join(&format!("/v1/ageRatingDeclarations/{id}"))
            .map_err(|_| Error::invalid_upstream())?;
        let payload = self.apple(url, Some(&json!({"data":{"type":"ageRatingDeclarations", "id":id, "attributes":attributes}})), &mut budget, route).await?;
        if payload["data"]["id"].as_str() != Some(id) {
            return Err(Error::unknown_write());
        }
        let context = serde_json::Map::from_iter([
            ("appId".into(), app_id.into()),
            ("appInfoId".into(), app_info_id.into()),
        ]);
        let flat = contract::flatten(&payload["data"], route, &context, None)
            .map_err(|_| Error::unknown_write())?;
        serde_json::from_value(flat).map_err(|_| Error::unknown_write())
    }

    /// Update the supported editable Store metadata on an existing app, version, or
    /// localization facade path. Reads the scoped resource first, then sends one Apple
    /// PATCH containing only `attributes`. Empty strings are sent through to clear values.
    /// Writes are never retried; read the affected resource before retrying an uncertain write.
    pub async fn update_metadata(&self, path: &str, attributes: &Value) -> Result<Value> {
        let (route, params) = contract::match_route(path)?;
        let supported = match route.handler.as_str() {
            "app" => &["contentRightsDeclaration"][..],
            "version" => &["versionString", "copyright"][..],
            "appInfoLocalization" => &["privacyPolicyUrl"][..],
            "versionLocalization" => &[
                "promotionalText",
                "description",
                "whatsNew",
                "keywords",
                "supportUrl",
                "marketingUrl",
            ][..],
            _ => &[][..],
        };
        let Some(values) = attributes.as_object() else {
            return Err(invalid_metadata("attributes"));
        };
        if values.is_empty()
            || supported.is_empty()
            || values
                .iter()
                .any(|(name, value)| !supported.contains(&name.as_str()) || !value.is_string())
        {
            return Err(invalid_metadata("attributes"));
        }

        let mut budget = transport::Budget::new();
        let current = self.read(path, &Query::new(), &mut budget).await?;
        let id = current["id"].as_str().ok_or_else(Error::invalid_upstream)?;
        let url = self
            .inner
            .origin
            .join(&format!("/v1/{}/{id}", route.resource_type))
            .map_err(|_| Error::invalid_upstream())?;
        let payload = self
            .apple(
                url,
                Some(&json!({
                    "data": {
                        "type": route.resource_type,
                        "id": id,
                        "attributes": attributes
                    }
                })),
                &mut budget,
                route,
            )
            .await?;
        let data = &payload["data"];
        if data["id"].as_str() != Some(id)
            || contract::resource(data, &route.resource_type).is_err()
        {
            return Err(Error::unknown_write());
        }

        let mut context = serde_json::Map::new();
        for key in ["appId", "appInfoId", "versionId"] {
            if let Some(value) = current.get(key) {
                context.insert(key.into(), value.clone());
            }
        }
        if let Some(locale) = params.get("locale") {
            if data["attributes"]["locale"].as_str() != Some(locale) {
                return Err(Error::unknown_write());
            }
            let (parent_relation, parent_id, parent_type) =
                if route.handler == "appInfoLocalization" {
                    ("appInfo", &params["appInfoId"], "appInfos")
                } else {
                    ("appStoreVersion", &params["versionId"], "appStoreVersions")
                };
            if contract::check_parent(data, parent_relation, parent_id, parent_type).is_err() {
                return Err(Error::unknown_write());
            }
        } else if route.handler == "version" {
            if contract::check_parent(data, "app", &params["appId"], "apps").is_err() {
                return Err(Error::unknown_write());
            }
        }

        let confirmed =
            contract::flatten(data, route, &context, None).map_err(|_| Error::unknown_write())?;
        let mut merged = current
            .as_object()
            .cloned()
            .ok_or_else(Error::unknown_write)?;
        for (name, value) in values {
            merged.insert(name.clone(), value.clone());
        }
        if let Some(confirmed) = confirmed.as_object() {
            merged.extend(confirmed.clone());
        } else {
            return Err(Error::unknown_write());
        }
        Ok(Value::Object(merged))
    }
}

fn invalid_metadata(field: &str) -> Error {
    let mut error = Error::new(400, "invalid_update", "The metadata update is invalid.");
    error.details.push(ErrorDetail {
        code: "invalid_attribute".into(),
        message: "Provide supported string attributes only.".into(),
        field: Some(field.into()),
    });
    error
}

#[cfg(test)]
mod tests;
