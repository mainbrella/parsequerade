use crate::{contract, transport::Budget, Client, Error, Query, Result};
use reqwest::{header, Method, Url};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const MAX_PNG: usize = 32 * 1024 * 1024;
const MAX_OPERATIONS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListingImagePlacementType {
    #[serde(rename = "PRODUCT_PAGE_HEADER_ASSET")]
    ProductPageHeader,
    #[serde(rename = "APP_STORE_SEARCH_RESULTS_ASSET")]
    SearchResults,
}

impl ListingImagePlacementType {
    fn apple(self) -> &'static str {
        match self {
            Self::ProductPageHeader => "PRODUCT_PAGE_HEADER_ASSET",
            Self::SearchResults => "APP_STORE_SEARCH_RESULTS_ASSET",
        }
    }
    fn dimensions(self) -> (u32, u32) {
        match self {
            Self::ProductPageHeader => (3840, 1646),
            Self::SearchResults => (1920, 1280),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingImageUpload {
    pub app_id: String,
    pub version_id: String,
    pub locale: String,
    pub placement_type: ListingImagePlacementType,
    pub file_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingImageUploadCheckpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placement_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingImageUploadReceipt {
    pub app_id: String,
    pub version_id: String,
    pub locale: String,
    pub placement_type: ListingImagePlacementType,
    pub image_id: String,
    pub placement_id: String,
}

fn failure(code: &'static str, message: &'static str) -> Error {
    Error::new(422, code, message)
}
fn unknown() -> Error {
    Error::unknown_write()
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or_else(Error::invalid_upstream)
}
fn api_url(client: &Client, path: &str) -> Result<Url> {
    client
        .inner
        .origin
        .join(path)
        .map_err(|_| Error::invalid_upstream())
}
fn sha(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
fn bind(request: &ListingImageUpload, png: &[u8]) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        request.app_id,
        request.version_id,
        request.locale,
        request.placement_type.apple(),
        request.file_name,
        sha(png)
    )
}
fn validate_png(request: &ListingImageUpload, png: &[u8]) -> Result<(u32, u32, bool)> {
    if png.len() < 45
        || png.len() > MAX_PNG
        || !png.starts_with(b"\x89PNG\r\n\x1a\n")
        || &png[12..16] != b"IHDR"
    {
        return Err(failure(
            "invalid_png",
            "Provide a complete PNG file within the 32 MiB limit.",
        ));
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    let mut alpha = matches!(png[25], 4 | 6);
    let expected = request.placement_type.dimensions();
    if (width, height) != expected {
        return Err(failure(
            "invalid_image_dimensions",
            "The PNG dimensions do not match the selected placement type.",
        ));
    }
    if png[24] != 8 || !matches!(png[25], 2 | 3 | 4 | 6) {
        return Err(failure(
            "invalid_png",
            "The PNG color format is not supported.",
        ));
    }
    let mut pos = 8usize;
    let mut ended = false;
    let mut saw_ihdr = false;
    let mut saw_idat = false;
    let mut saw_iend = false;
    while pos.checked_add(12).is_some_and(|end| end <= png.len()) {
        let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
        let end = pos
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .ok_or_else(|| failure("invalid_png", "The PNG chunk layout is invalid."))?;
        if end > png.len() {
            return Err(failure("invalid_png", "The PNG chunk layout is invalid."));
        }
        let kind = &png[pos + 4..pos + 8];
        let expected_crc = u32::from_be_bytes(png[end - 4..end].try_into().unwrap());
        if crc32(&png[pos + 4..end - 4]) != expected_crc {
            return Err(failure("invalid_png", "The PNG chunk checksum is invalid."));
        }
        if kind == b"IHDR" {
            if saw_ihdr || pos != 8 || len != 13 {
                return Err(failure("invalid_png", "The PNG header is invalid."));
            }
            saw_ihdr = true;
        }
        if kind == b"IDAT" {
            saw_idat = true;
        }
        if kind == b"tRNS" {
            alpha = true;
        }
        if kind == b"IEND" {
            saw_iend = true;
            ended = len == 0 && end == png.len();
            break;
        }
        pos = end;
    }
    if !ended || !saw_ihdr || !saw_idat || !saw_iend {
        return Err(failure(
            "invalid_png",
            "The PNG must include valid image data and a final IEND chunk.",
        ));
    }
    Ok((width, height, alpha))
}

fn resolve_placement_group(
    data: &Value,
    placement_type: &str,
    width: u32,
    height: u32,
    size: usize,
    alpha: bool,
) -> Result<String> {
    let attrs = data["data"][0]["attributes"]
        .as_object()
        .ok_or_else(Error::invalid_upstream)?;
    let arr = |name: &str| {
        attrs
            .get(name)
            .and_then(Value::as_array)
            .ok_or_else(Error::invalid_upstream)
    };
    let feature = arr("features")?
        .iter()
        .find(|v| v["featureId"] == "APP_STORE_VERSIONS")
        .ok_or_else(|| {
            failure(
                "placement_unavailable",
                "Apple does not expose App Store version placements for this key.",
            )
        })?;
    let policy = feature["placementPolicies"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["placementType"] == placement_type))
        .ok_or_else(|| {
            failure(
                "placement_unavailable",
                "Apple does not support this placement type for the selected surface.",
            )
        })?;
    let allowed: BTreeSet<&str> = policy["groupLimits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| g["maxCount"].as_u64().unwrap_or(0) >= 1)
        .flat_map(|g| g["groupIds"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let groups = arr("placementProfileGroups")?;
    let types = arr("placementTypes")?;
    let typ = types
        .iter()
        .find(|v| v["placementTypeId"] == placement_type)
        .ok_or_else(Error::invalid_upstream)?;
    if !typ["acceptsAssetCategories"]
        .as_array()
        .is_some_and(|v| v.iter().any(|c| c == "CREATIVE_ASSETS"))
    {
        return Err(failure(
            "placement_unavailable",
            "Apple does not accept creative image assets for this placement.",
        ));
    }
    let specs = arr("imageSpecs")?;
    let mut matches = BTreeSet::new();
    for group in groups.iter().filter(|g| {
        allowed.contains(g["placementProfileGroupId"].as_str().unwrap_or(""))
            && g["platform"] == "IOS"
    }) {
        let gid = group["placementProfileGroupId"]
            .as_str()
            .ok_or_else(Error::invalid_upstream)?;
        let mapping = typ["specMappings"]
            .as_array()
            .and_then(|a| a.iter().find(|m| m["placementGroupId"] == gid));
        let Some(spec_ids) = mapping.and_then(|m| m["specs"].as_array()) else {
            continue;
        };
        let valid = spec_ids
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|id| specs.iter().find(|s| s["specId"] == id))
            .any(|spec| {
                let d = &spec["dimensions"];
                d["minWidth"].as_u64() == Some(width as u64)
                    && d["maxWidth"].as_u64() == Some(width as u64)
                    && d["minHeight"].as_u64() == Some(height as u64)
                    && d["maxHeight"].as_u64() == Some(height as u64)
                    && spec["maxFileSize"]
                        .as_u64()
                        .is_some_and(|m| size as u64 <= m)
                    && spec["fileExtensions"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|v| v == ".png"))
                    && spec["mimeTypes"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|v| v == "image/png"))
                    && (!alpha || spec["alphaAllowed"] == true)
            });
        if valid {
            matches.insert(gid.to_owned());
        }
    }
    if matches.len() != 1 {
        return Err(failure("placement_ambiguous", "Apple's current specifications do not resolve to exactly one compatible placement group."));
    }
    Ok(matches.into_iter().next().unwrap())
}

async fn raw(client: &Client, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
    let mut budget = Budget::new();
    let route = crate::contract::route("placements");
    client
        .apple_method(api_url(client, path)?, method, body, &mut budget, route)
        .await
}

#[allow(unused_variables)]
fn operations(
    client: &Client,
    image: &Value,
    png: &[u8],
) -> Result<
    Vec<(
        Url,
        usize,
        usize,
        Vec<(header::HeaderName, header::HeaderValue)>,
    )>,
> {
    let arr = image["data"]["attributes"]["uploadOperations"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= MAX_OPERATIONS)
        .ok_or_else(Error::invalid_upstream)?;
    let mut result = Vec::new();
    let mut ranges = Vec::new();
    for op in arr {
        if op["method"] != "PUT" {
            return Err(Error::invalid_upstream());
        }
        let url = Url::parse(text(op, "url")?).map_err(|_| Error::invalid_upstream())?;
        let host = url.host_str().unwrap_or("");
        let apple_host = url.scheme() == "https"
            && (host == "blobstore.apple.com" || host.ends_with(".blobstore.apple.com"))
            && url.port().is_none();
        #[cfg(test)]
        let test_origin = url.origin() == client.inner.origin.origin();
        #[cfg(not(test))]
        let test_origin = false;
        if !(apple_host || test_origin)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::invalid_upstream());
        }
        let offset = op["offset"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(Error::invalid_upstream)?;
        let length = op["length"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(Error::invalid_upstream)?;
        if offset.checked_add(length).is_none_or(|end| end > png.len()) {
            return Err(Error::invalid_upstream());
        }
        let mut headers = Vec::new();
        let mut names = BTreeSet::new();
        let operation_headers = op["requestHeaders"]
            .as_array()
            .filter(|v| v.len() <= 32)
            .ok_or_else(Error::invalid_upstream)?;
        for h in operation_headers {
            let name = text(h, "name")?.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "authorization"
                    | "proxy-authorization"
                    | "cookie"
                    | "host"
                    | "transfer-encoding"
                    | "connection"
            ) || !names.insert(name.clone())
            {
                return Err(Error::invalid_upstream());
            }
            let value = text(h, "value")?;
            let name = header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::invalid_upstream())?;
            let value =
                header::HeaderValue::from_str(value).map_err(|_| Error::invalid_upstream())?;
            if name == header::CONTENT_LENGTH
                && value.to_str().ok().and_then(|v| v.parse::<usize>().ok()) != Some(length)
            {
                return Err(Error::invalid_upstream());
            }
            headers.push((name, value));
        }
        ranges.push((offset, offset + length));
        result.push((url, offset, length, headers));
    }
    ranges.sort_unstable();
    let mut end = 0;
    for (start, next) in ranges {
        if start != end {
            return Err(Error::invalid_upstream());
        }
        end = next;
    }
    if end != png.len() {
        return Err(Error::invalid_upstream());
    }
    Ok(result)
}

async fn transfer(
    client: &Client,
    ops: Vec<(
        Url,
        usize,
        usize,
        Vec<(header::HeaderName, header::HeaderValue)>,
    )>,
    png: &[u8],
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    for (url, offset, length, headers) in ops {
        let timeout =
            Duration::from_secs(60).min(deadline.saturating_duration_since(Instant::now()));
        if timeout.is_zero() {
            return Err(unknown());
        }
        let mut req = client
            .inner
            .http
            .put(url)
            .timeout(timeout)
            .body(png[offset..offset + length].to_vec());
        for (name, value) in headers {
            req = req.header(name, value);
        }
        let response = tokio::time::timeout(timeout, req.send())
            .await
            .map_err(|_| unknown())?
            .map_err(|_| unknown())?;
        if !response.status().is_success() {
            return Err(unknown());
        }
    }
    Ok(())
}

impl Client {
    /// Upload one PNG for a version localization and create its listing image placement.
    /// The persistence callback runs before each write and after recording each returned ID.
    pub async fn upload_listing_image<F>(
        &self,
        request: &ListingImageUpload,
        png: &[u8],
        checkpoint: &mut ListingImageUploadCheckpoint,
        mut persist: F,
    ) -> Result<ListingImageUploadReceipt>
    where
        F: FnMut(&ListingImageUploadCheckpoint) -> Result<()>,
    {
        contract::validate_id(&request.app_id)?;
        contract::validate_id(&request.version_id)?;
        contract::validate_locale(&request.locale)?;
        if request.file_name.is_empty()
            || request.file_name.len() > 255
            || request.file_name.contains('/')
            || request.file_name.contains('\\')
            || !request.file_name.to_ascii_lowercase().ends_with(".png")
        {
            return Err(failure(
                "invalid_file_name",
                "Provide a PNG filename without a path.",
            ));
        }
        let (w, h, alpha) = validate_png(request, png)?;
        let binding = bind(request, png);
        if checkpoint.binding.as_deref().is_some_and(|v| v != binding) {
            return Err(failure(
                "checkpoint_mismatch",
                "The checkpoint belongs to a different request or PNG.",
            ));
        }
        if checkpoint.binding.is_none() {
        checkpoint.binding = Some(binding.clone());
        }
        let localization = self
            .version_localization(
                &request.app_id,
                &request.version_id,
                &request.locale,
                &Query::new(),
            )
            .await?;
        contract::validate_id(&localization.id)?;
        if localization.locale.as_deref() != Some(request.locale.as_str()) {
            return Err(Error::invalid_upstream());
        }
        let placements = self
            .placements(
                &request.app_id,
                &request.version_id,
                &request.locale,
                &Query::new()
                    .with("placementType", request.placement_type.apple())
                    .with("limit", 200),
            )
            .await?;
        if placements.next_cursor.is_some() {
            return Err(Error::incomplete());
        }
        let matching: Vec<_> = placements
            .items
            .iter()
            .filter(|p| p.placement_type.as_deref() == Some(request.placement_type.apple()))
            .collect();
        if matching.len() > 1 { return Err(Error::incomplete()); }
        if let Some(p) = matching.first().copied() {
            if checkpoint.image_id.as_deref() == p.image_id.as_deref()
                && p.app_store_version_localization_id.as_deref() == Some(localization.id.as_str())
                && checkpoint.phase.as_deref().is_some_and(|v| matches!(v, "placement_started" | "placement_created"))
                && checkpoint.placement_group.as_deref() == p.placement_group.as_deref()
                && p.state.as_deref() != Some("FAILED")
            {
                if checkpoint.placement_id.as_deref().is_some_and(|id| id != p.id) { return Err(unknown()); }
                checkpoint.placement_id = Some(p.id.clone());
                checkpoint.phase = Some("placement_created".into());
                persist(checkpoint)?;
                return Ok(receipt(
                    request,
                    checkpoint.image_id.as_ref().unwrap(),
                    &p.id,
                ));
            }
            return Err(failure(
                "existing_placement",
                "A placement already exists for this type; the SDK will not replace it.",
            ));
        }
        if checkpoint.phase.as_deref() == Some("placement_started") { return Err(unknown()); }
        let lib = raw(
            self,
            Method::GET,
            &format!("/v1/apps/{}/assetLibrary", request.app_id),
            None,
        )
        .await?;
        if text(&lib["data"], "type")? != "appAssetLibraries" {
            return Err(Error::invalid_upstream());
        }
        let library_id = text(&lib["data"], "id")?;
        let refdata = raw(self, Method::GET, "/v1/appAssetLibraryRefData", None).await?;
        let group = resolve_placement_group(
            &refdata,
            request.placement_type.apple(),
            w,
            h,
            png.len(),
            alpha,
        )?;
        if checkpoint.image_id.is_none() {
            if checkpoint.phase.as_deref() == Some("reservation_started") {
                return Err(unknown());
            }
            checkpoint.phase = Some("reservation_started".into());
            persist(checkpoint)?;
            let payload = json!({"data":{"type":"appAssetLibraryImages","attributes":{"fileName":request.file_name,"fileSize":png.len(),"category":"CREATIVE_ASSETS","referenceName":format!("parsequerade-{}",sha(&binding.as_bytes()))},"relationships":{"assetLibrary":{"data":{"type":"appAssetLibraries","id":library_id}}}}});
            let response = raw(
                self,
                Method::POST,
                "/v1/appAssetLibraryImages",
                Some(&payload),
            )
            .await?;
            if response["data"]["type"] != "appAssetLibraryImages"
                || response["data"]["attributes"]["state"] != "AWAITING_UPLOAD"
            {
                return Err(unknown());
            }
            checkpoint.image_id = Some(text(&response["data"], "id")?.to_owned());
            checkpoint.phase = Some("reserved".into());
            persist(checkpoint)?;
            let ops = operations(self, &response, png)?;
            transfer(self, ops, png).await?;
            checkpoint.phase = Some("commit_started".into());
            persist(checkpoint)?;
            let commit = json!({"data":{"type":"appAssetLibraryImages","id":checkpoint.image_id,"attributes":{"uploaded":true}}});
            let committed = raw(
                self,
                Method::PATCH,
                &format!(
                    "/v1/appAssetLibraryImages/{}",
                    checkpoint.image_id.as_ref().unwrap()
                ),
                Some(&commit),
            )
            .await?;
            if committed["data"]["id"] != checkpoint.image_id.as_deref().unwrap_or("")
                || committed["data"]["type"] != "appAssetLibraryImages"
            {
                return Err(unknown());
            }
            checkpoint.phase = Some("committed".into());
            persist(checkpoint)?;
        } else {
            let image_id = checkpoint.image_id.as_ref().unwrap();
            if checkpoint.phase.as_deref() == Some("reservation_started") {
                return Err(unknown());
            }
            let image = raw(
                self,
                Method::GET,
                &format!("/v1/appAssetLibraryImages/{image_id}"),
                None,
            )
            .await?;
            if image["data"]["id"] != image_id.as_str()
                || image["data"]["type"] != "appAssetLibraryImages"
            {
                return Err(Error::invalid_upstream());
            }
            match image["data"]["attributes"]["state"].as_str() {
                Some("AWAITING_UPLOAD")
                    if checkpoint.phase.as_deref() != Some("commit_started") =>
                {
                    let ops = operations(self, &image, png)?;
                    transfer(self, ops, png).await?;
                    checkpoint.phase = Some("commit_started".into());
                    persist(checkpoint)?;
                    let body = json!({"data":{"type":"appAssetLibraryImages","id":image_id,"attributes":{"uploaded":true}}});
                    let c = raw(
                        self,
                        Method::PATCH,
                        &format!("/v1/appAssetLibraryImages/{image_id}"),
                        Some(&body),
                    )
                    .await?;
                    if c["data"]["id"] != image_id.as_str() {
                        return Err(unknown());
                    }
                    checkpoint.phase = Some("committed".into());
                    persist(checkpoint)?;
                }
                Some("AWAITING_UPLOAD") => return Err(unknown()),
                Some("UPLOAD_COMPLETE" | "PREPARE_FOR_SUBMISSION") => {}
                _ => {
                    return Err(failure(
                        "image_not_ready",
                        "Apple has not made this image ready for placement.",
                    ))
                }
            }
        }
        let image_id = checkpoint.image_id.as_ref().unwrap().clone();
        checkpoint.phase = Some("placement_started".into());
        persist(checkpoint)?;
        let body = json!({"data":{"type":"appAssetLibraryPlacements","attributes":{"placementType":request.placement_type.apple(),"placementGroup":group},"relationships":{"image":{"data":{"type":"appAssetLibraryImages","id":image_id}},"appStoreVersionLocalization":{"data":{"type":"appStoreVersionLocalizations","id":localization.id}}}}});
        let placement = raw(
            self,
            Method::POST,
            "/v1/appAssetLibraryPlacements",
            Some(&body),
        )
        .await?;
        if placement["data"]["type"] != "appAssetLibraryPlacements" {
            return Err(unknown());
        }
        checkpoint.placement_id = Some(text(&placement["data"], "id")?.to_owned());
        checkpoint.phase = Some("placement_created".into());
        persist(checkpoint)?;
        let verified = self
            .placements(
                &request.app_id,
                &request.version_id,
                &request.locale,
                &Query::new()
                    .with("placementType", request.placement_type.apple())
                    .with("limit", 200),
            )
            .await?;
        let found = verified
            .items
            .iter()
            .find(|p| {
                p.id == checkpoint.placement_id.as_deref().unwrap_or("")
                    && p.image_id.as_deref() == Some(image_id.as_str())
                    && p.app_store_version_localization_id.as_deref()
                        == Some(localization.id.as_str())
                    && p.placement_type.as_deref() == Some(request.placement_type.apple())
                    && p.placement_group.as_deref() == Some(group.as_str())
                    && p.state.as_deref() != Some("FAILED")
            })
            .ok_or_else(unknown)?;
        Ok(receipt(request, &image_id, &found.id))
    }
}
fn receipt(
    r: &ListingImageUpload,
    image_id: &str,
    placement_id: &str,
) -> ListingImageUploadReceipt {
    ListingImageUploadReceipt {
        app_id: r.app_id.clone(),
        version_id: r.version_id.clone(),
        locale: r.locale.clone(),
        placement_type: r.placement_type,
        image_id: image_id.into(),
        placement_id: placement_id.into(),
    }
}
