use crate::{contract, transport::Budget, Client, Error, Query, Result};
use serde_json::{json, Value};

impl Client {
    /// Attach a processed App Store build to an exact draft version in the same app.
    /// Sends one PATCH, never retries a write, and reads back the build to confirm it.
    pub async fn attach_build(
        &self,
        app_id: &str,
        version_id: &str,
        build_id: &str,
    ) -> Result<Value> {
        for id in [app_id, version_id, build_id] {
            contract::validate_id(id)?;
        }
        let mut budget = Budget::new();
        let path = format!("/apps/{app_id}/versions/{version_id}");
        let mut version = self.read(&path, &Query::new(), &mut budget).await?;
        if version["appVersionState"] != "PREPARE_FOR_SUBMISSION" {
            return Err(Error::new(
                409,
                "version_not_editable",
                "Select a Prepare for Submission version.",
            ));
        }
        let build_path = format!("/apps/{app_id}/builds/{build_id}");
        let build = self.read(&build_path, &Query::new(), &mut budget).await?;
        if build["processingState"] != "VALID"
            || build["expired"] != false
            || build["buildAudienceType"] == "INTERNAL_ONLY"
        {
            return Err(Error::new(
                409,
                "build_not_available",
                "Select a processed, unexpired App Store build.",
            ));
        }
        let prerelease = self
            .read(
                &format!("{build_path}/prerelease-version"),
                &Query::new(),
                &mut budget,
            )
            .await?;
        if prerelease["platform"] != version["platform"]
            || prerelease["version"].as_str().filter(|s| !s.is_empty())
                != version["versionString"].as_str()
        {
            return Err(Error::new(
                409,
                "build_version_mismatch",
                "The build must match the selected version and platform.",
            ));
        }
        let body = json!({"data": {
            "type": "appStoreVersions", "id": version_id,
            "relationships": {"build": {"data": {"type": "builds", "id": build_id}}}
        }});
        let url = self
            .inner
            .origin
            .join(&format!("/v1/appStoreVersions/{version_id}"))
            .map_err(|_| Error::invalid_upstream())?;
        let payload = self
            .apple(url, Some(&body), &mut budget, contract::route("version"))
            .await?;
        if contract::resource(&payload["data"], "appStoreVersions").is_err()
            || payload["data"]["id"] != version_id
            || contract::check_parent(&payload["data"], "app", app_id, "apps").is_err()
        {
            return Err(Error::unknown_write());
        }
        let confirmed = self
            .read(&format!("{path}/build"), &Query::new(), &mut budget)
            .await
            .map_err(|_| Error::unknown_write())?;
        if confirmed["id"] != build_id {
            return Err(Error::unknown_write());
        }
        version["buildId"] = build_id.into();
        Ok(version)
    }
}
