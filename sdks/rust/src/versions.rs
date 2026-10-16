use crate::{contract, transport::Budget, Client, Error, Query, Result};
use serde_json::{json, Map, Value};

#[cfg(test)]
mod tests;

impl Client {
    /// Create an unsubmitted App Store version. Sends one POST and never retries.
    /// On `write_outcome_unknown`, list versions before deciding to try again.
    pub async fn create_version(
        &self,
        app_id: &str,
        platform: &str,
        version_string: &str,
    ) -> Result<Value> {
        contract::validate_id(app_id)?;
        if !matches!(platform, "IOS" | "MAC_OS" | "TV_OS" | "VISION_OS")
            || version_string.is_empty()
            || version_string.len() > 18
            || version_string.split('.').count() > 3
            || version_string
                .split('.')
                .any(|part| part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()))
        {
            return Err(Error::new(400, "invalid_request", "Provide a supported platform and numeric version string."));
        }
        let mut budget = Budget::new();
        self.read(&format!("/apps/{app_id}"), &Query::new(), &mut budget).await?;
        let route = contract::route("versions");
        let url = self.inner.origin.join("/v1/appStoreVersions")
            .map_err(|_| Error::invalid_upstream())?;
        let body = json!({"data": {
            "type": "appStoreVersions",
            "attributes": {"platform": platform, "versionString": version_string},
            "relationships": {"app": {"data": {"type": "apps", "id": app_id}}}
        }});
        let payload = self.apple_method(url, reqwest::Method::POST, Some(&body), &mut budget, route).await?;
        contract::check_parent(&payload["data"], "app", app_id, "apps")
            .map_err(|_| Error::unknown_write())?;
        let context = Map::from_iter([("appId".into(), app_id.into())]);
        let flat = contract::flatten(&payload["data"], route, &context, None)
            .map_err(|_| Error::unknown_write())?;
        if flat["platform"] != platform || flat["versionString"] != version_string
            || flat["appVersionState"] != "PREPARE_FOR_SUBMISSION"
        {
            return Err(Error::unknown_write());
        }
        Ok(flat)
    }
}
