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

include!("upload_models.rs");

fn failure(code: &'static str, message: &'static str) -> Error {
    Error::new(422, code, message)
}
fn unknown() -> Error {
    Error::new(
        503,
        "write_outcome_unknown",
        "Apple did not confirm this upload. Resume with the same checkpoint to reconcile its state.",
    )
}
fn image_usable(state: Option<&str>) -> bool {
    matches!(
        state,
        Some(
            "UPLOAD_COMPLETE"
                | "PREPARE_FOR_SUBMISSION"
                | "READY_FOR_REVIEW"
                | "WAITING_FOR_REVIEW"
                | "IN_REVIEW"
                | "ACCEPTED"
                | "APPROVED"
        )
    )
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
    if data["data"].as_array().is_none_or(|items| items.len() != 1)
        || data["data"][0]["type"] != "appAssetLibraryRefData"
        || !data["links"]["next"].is_null()
    {
        return Err(Error::invalid_upstream());
    }
    contract::validate_id(
        data["data"][0]["id"]
            .as_str()
            .ok_or_else(Error::invalid_upstream)?,
    )
    .map_err(|_| Error::invalid_upstream())?;
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
            && matches!(g["platform"].as_str(), Some("IOS" | "ANY"))
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
            .any(|spec| image_spec_matches(spec, placement_type, width, height, size, alpha));
        if valid {
            matches.insert(gid.to_owned());
        }
    }
    if matches.is_empty() {
        return Err(failure(
            "placement_spec_not_found",
            "Apple's current catalog has no compatible image specification for this PNG and placement type.",
        ));
    }
    if matches.len() > 1 {
        return Err(failure(
            "placement_ambiguous",
            "Apple's current catalog resolves this PNG to more than one compatible placement group.",
        ));
    }
    Ok(matches.into_iter().next().unwrap())
}

fn image_spec_matches(
    spec: &Value,
    placement_type: &str,
    width: u32,
    height: u32,
    size: usize,
    alpha: bool,
) -> bool {
    let dimensions = &spec["dimensions"];
    let (Some(min_width), Some(max_width), Some(min_height), Some(max_height)) = (
        dimensions["minWidth"].as_u64(),
        dimensions["maxWidth"].as_u64(),
        dimensions["minHeight"].as_u64(),
        dimensions["maxHeight"].as_u64(),
    ) else {
        return false;
    };
    if min_width > max_width
        || min_height > max_height
        || !(min_width..=max_width).contains(&(width as u64))
        || !(min_height..=max_height).contains(&(height as u64))
        || !spec["compatiblePlacementTypes"]
            .as_array()
            .is_some_and(|types| types.iter().any(|kind| kind == placement_type))
        || !spec["maxFileSize"]
            .as_u64()
            .is_some_and(|limit| size as u64 <= limit)
        || !spec["fileExtensions"]
            .as_array()
            .is_some_and(|extensions| extensions.iter().any(|extension| extension == ".png"))
        || !spec["mimeTypes"]
            .as_array()
            .is_some_and(|mime_types| mime_types.iter().any(|mime| mime == "image/png"))
        || (alpha && spec["alphaAllowed"] != true)
    {
        return false;
    }

    let exact_size = min_width == max_width && min_height == max_height;
    exact_size || aspect_ratio_matches(&spec["aspectRatio"], width, height)
}

fn aspect_ratio_matches(value: &Value, width: u32, height: u32) -> bool {
    let Some(ratio) = value.as_str() else {
        return false;
    };
    let Some((numerator, denominator)) = ratio.split_once(':') else {
        return false;
    };
    let (Ok(numerator), Ok(denominator)) = (numerator.parse::<u64>(), denominator.parse::<u64>())
    else {
        return false;
    };
    if numerator == 0 || denominator == 0 {
        return false;
    }
    let Some(actual) = u128::from(width).checked_mul(u128::from(denominator)) else {
        return false;
    };
    let Some(expected) = u128::from(height).checked_mul(u128::from(numerator)) else {
        return false;
    };
    actual.abs_diff(expected) <= u128::from(numerator.max(denominator))
}

type UploadOperation = (
    Url,
    usize,
    usize,
    Vec<(header::HeaderName, header::HeaderValue)>,
);

fn upload_destination_allowed(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("");
    let apple_host = host == "blobstore.apple.com"
        || host.ends_with(".blobstore.apple.com")
        || host == "object-storage.apple.com"
        || host.ends_with(".object-storage.apple.com");
    url.scheme() == "https"
        && apple_host
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

async fn raw(client: &Client, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
    let mut budget = Budget::new();
    let route = crate::contract::route("placements");
    client
        .apple_method(api_url(client, path)?, method, body, &mut budget, route)
        .await
}

fn operations(_client: &Client, image: &Value, png: &[u8]) -> Result<Vec<UploadOperation>> {
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
        let apple_destination = upload_destination_allowed(&url);
        #[cfg(test)]
        let test_origin = url.origin() == _client.inner.origin.origin();
        #[cfg(not(test))]
        let test_origin = false;
        if !(apple_destination || test_origin) {
            let mut error = Error::invalid_upstream();
            error.message = "Apple returned an unsupported upload destination.".into();
            return Err(error);
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

async fn transfer(client: &Client, ops: Vec<UploadOperation>, png: &[u8]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    for (url, offset, length, headers) in ops {
        let timeout =
            Duration::from_secs(60).min(deadline.saturating_duration_since(Instant::now()));
        if timeout.is_zero() {
            return Err(unknown());
        }
        let headers = headers
            .into_iter()
            .map(|(name, value)| {
                Ok((
                    name.to_string(),
                    value.to_str().map_err(|_| unknown())?.to_owned(),
                ))
            })
            .collect::<Result<_>>()?;
        let response = client
            .send_http(crate::HttpRequest {
                method: "PUT".into(),
                url: url.to_string(),
                headers,
                body: Some(png[offset..offset + length].to_vec()),
                timeout,
                max_response_bytes: 64 * 1024,
            })
            .await
            .map_err(|_| unknown())?;
        if !(200..300).contains(&response.status) {
            return Err(unknown());
        }
    }
    Ok(())
}

impl Client {
    /// Read Apple's live asset specifications and placement groups without making writes.
    /// The returned value preserves Apple's JSON:API envelope for diagnostics.
    pub async fn listing_image_reference_data(&self) -> Result<Value> {
        raw(self, Method::GET, "/v1/appAssetLibraryRefData", None).await
    }

    /// Upload one PNG for a version localization and create its listing image placement.
    /// The persistence callback runs before each write and after recording each returned ID.
    /// It must durably save the checkpoint before returning `Ok(())`; serialize access to each
    /// checkpoint so another caller cannot start a concurrent upload from stale state.
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
        if checkpoint.binding.is_none()
            && (checkpoint.phase.is_some()
                || checkpoint.image_id.is_some()
                || checkpoint.placement_id.is_some()
                || checkpoint.placement_group.is_some())
        {
            return Err(failure(
                "invalid_checkpoint",
                "An in-progress checkpoint must include its request binding.",
            ));
        }
        if checkpoint.binding.as_deref().is_some_and(|v| v != binding) {
            return Err(failure(
                "checkpoint_mismatch",
                "The checkpoint belongs to a different request or PNG.",
            ));
        }
        if checkpoint.binding.is_none() {
            checkpoint.binding = Some(binding.clone());
        }
        let valid_phase = matches!(
            checkpoint.phase.as_deref(),
            None | Some("reservation_started")
                | Some("reserved")
                | Some("upload_started")
                | Some("commit_started")
                | Some("committed")
                | Some("placement_started")
                | Some("placement_created")
        );
        if !valid_phase
            || (checkpoint.phase.is_none() && checkpoint.image_id.is_some())
            || (checkpoint.phase.as_deref() == Some("reservation_started")
                && checkpoint.image_id.is_some())
            || (checkpoint
                .phase
                .as_deref()
                .is_some_and(|p| p != "reservation_started")
                && checkpoint.image_id.is_none())
            || (checkpoint.placement_id.is_some() && checkpoint.image_id.is_none())
            || (checkpoint.placement_group.is_some()
                && !matches!(
                    checkpoint.phase.as_deref(),
                    Some("placement_started" | "placement_created")
                ))
        {
            return Err(failure(
                "invalid_checkpoint",
                "The upload checkpoint state is invalid.",
            ));
        }
        for id in [
            checkpoint.image_id.as_deref(),
            checkpoint.placement_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            contract::validate_id(id).map_err(|_| {
                failure(
                    "invalid_checkpoint",
                    "The upload checkpoint contains an invalid ID.",
                )
            })?;
        }
        if checkpoint.phase.as_deref() == Some("reservation_started")
            && checkpoint.image_id.is_none()
        {
            return Err(unknown());
        }
        let localization = self
            .version_localization(
                &request.app_id,
                &request.version_id,
                &request.locale,
                &Query::new().with("fields", "locale"),
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
        if matching.len() > 1 {
            return Err(Error::incomplete());
        }
        if let Some(p) = matching.first().copied() {
            if checkpoint.image_id.as_deref() == p.image_id.as_deref()
                && p.app_store_version_localization_id.as_deref() == Some(localization.id.as_str())
                && checkpoint
                    .phase
                    .as_deref()
                    .is_some_and(|v| matches!(v, "placement_started" | "placement_created"))
                && checkpoint.placement_group.as_deref() == p.placement_group.as_deref()
                && p.state.as_deref() != Some("FAILED")
            {
                let image_id = checkpoint.image_id.as_deref().ok_or_else(unknown)?;
                let image = raw(
                    self,
                    Method::GET,
                    &format!("/v1/appAssetLibraryImages/{image_id}"),
                    None,
                )
                .await?;
                if image["data"]["type"] != "appAssetLibraryImages"
                    || image["data"]["id"] != image_id
                    || !image_usable(image["data"]["attributes"]["state"].as_str())
                {
                    return Err(unknown());
                }
                if checkpoint
                    .placement_id
                    .as_deref()
                    .is_some_and(|id| id != p.id)
                {
                    return Err(unknown());
                }
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
        if checkpoint
            .phase
            .as_deref()
            .is_some_and(|phase| matches!(phase, "placement_started" | "placement_created"))
        {
            return Err(unknown());
        }
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
        contract::validate_id(library_id)?;
        let refdata = raw(self, Method::GET, "/v1/appAssetLibraryRefData", None).await?;
        let group = resolve_placement_group(
            &refdata,
            request.placement_type.apple(),
            w,
            h,
            png.len(),
            alpha,
        )?;
        if checkpoint
            .placement_group
            .as_deref()
            .is_some_and(|old| old != group)
        {
            return Err(failure(
                "checkpoint_mismatch",
                "The placement group resolved differently from the checkpoint.",
            ));
        }
        if checkpoint.image_id.is_none() {
            if checkpoint.phase.as_deref() == Some("reservation_started") {
                return Err(unknown());
            }
            checkpoint.phase = Some("reservation_started".into());
            persist(checkpoint)?;
            let payload = json!({"data":{"type":"appAssetLibraryImages","attributes":{"fileName":request.file_name,"fileSize":png.len(),"category":"CREATIVE_ASSETS","referenceName":format!("parsequerade-{}",sha(binding.as_bytes()))},"relationships":{"assetLibrary":{"data":{"type":"appAssetLibraries","id":library_id}}}}});
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
            let image_id = text(&response["data"], "id").map_err(|_| unknown())?;
            contract::validate_id(image_id).map_err(|_| unknown())?;
            checkpoint.image_id = Some(image_id.to_owned());
            checkpoint.phase = Some("reserved".into());
            persist(checkpoint)?;
            let ops = operations(self, &response, png)?;
            checkpoint.phase = Some("upload_started".into());
            persist(checkpoint)?;
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
                || !matches!(
                    committed["data"]["attributes"]["state"].as_str(),
                    Some("UPLOAD_COMPLETE" | "PREPARE_FOR_SUBMISSION")
                )
            {
                return Err(unknown());
            }
            checkpoint.phase = Some("committed".into());
            persist(checkpoint)?;
        } else {
            let image_id = checkpoint.image_id.as_ref().ok_or_else(unknown)?;
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
            contract::validate_id(image_id)?;
            match image["data"]["attributes"]["state"].as_str() {
                Some("AWAITING_UPLOAD")
                    if matches!(
                        checkpoint.phase.as_deref(),
                        Some("reserved" | "upload_started")
                    ) =>
                {
                    let ops = operations(self, &image, png)?;
                    checkpoint.phase = Some("upload_started".into());
                    persist(checkpoint)?;
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
                    if c["data"]["id"] != image_id.as_str()
                        || c["data"]["type"] != "appAssetLibraryImages"
                        || !matches!(
                            c["data"]["attributes"]["state"].as_str(),
                            Some("UPLOAD_COMPLETE" | "PREPARE_FOR_SUBMISSION")
                        )
                    {
                        return Err(unknown());
                    }
                    checkpoint.phase = Some("committed".into());
                    persist(checkpoint)?;
                }
                Some("AWAITING_UPLOAD") => return Err(unknown()),
                Some(
                    "UPLOAD_COMPLETE"
                    | "PREPARE_FOR_SUBMISSION"
                    | "READY_FOR_REVIEW"
                    | "WAITING_FOR_REVIEW"
                    | "IN_REVIEW"
                    | "ACCEPTED"
                    | "APPROVED",
                ) => {}
                _ => {
                    return Err(failure(
                        "image_not_ready",
                        "Apple has not made this image ready for placement.",
                    ))
                }
            }
        }
        let image_id = checkpoint.image_id.as_ref().unwrap().clone();
        let latest = self
            .placements(
                &request.app_id,
                &request.version_id,
                &request.locale,
                &Query::new()
                    .with("placementType", request.placement_type.apple())
                    .with("limit", 200),
            )
            .await?;
        if latest.next_cursor.is_some()
            || latest
                .items
                .iter()
                .any(|p| p.placement_type.as_deref() == Some(request.placement_type.apple()))
        {
            return Err(failure(
                "existing_placement",
                "A placement appeared during this upload; the SDK will not replace it.",
            ));
        }
        checkpoint.placement_group = Some(group.clone());
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
        let placement_id = text(&placement["data"], "id").map_err(|_| unknown())?;
        contract::validate_id(placement_id).map_err(|_| unknown())?;
        checkpoint.placement_id = Some(placement_id.to_owned());
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

#[cfg(test)]
mod reference_data_tests {
    use super::resolve_placement_group;
    use serde_json::{json, Value};

    fn catalog() -> Value {
        serde_json::from_str(include_str!("tests/fixtures/listing-reference-data.json")).unwrap()
    }

    fn resolve(
        data: &Value,
        placement: &str,
        width: u32,
        height: u32,
        alpha: bool,
    ) -> crate::Result<String> {
        resolve_placement_group(data, placement, width, height, 2_600_000, alpha)
    }

    #[test]
    fn live_catalog_resolves_header_and_variable_search_dimensions() {
        let data = catalog();
        assert_eq!(
            resolve(&data, "PRODUCT_PAGE_HEADER_ASSET", 3840, 1646, false).unwrap(),
            "DEFAULT_PROFILE"
        );
        assert_eq!(
            resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 1920, 1280, false).unwrap(),
            "DEFAULT_PROFILE"
        );
        assert_eq!(
            resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 3840, 2560, false).unwrap(),
            "DEFAULT_PROFILE"
        );
    }

    #[test]
    fn ranged_specs_enforce_bounds_ratio_compatibility_and_asset_constraints() {
        let data = catalog();
        assert!(resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 1919, 1280, false).is_err());
        assert!(resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 1920, 1400, false).is_err());
        assert!(resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 1920, 1280, true).is_err());

        let mut incompatible = data.clone();
        let specs = incompatible["data"][0]["attributes"]["imageSpecs"]
            .as_array_mut()
            .unwrap();
        let search_spec = specs
            .iter_mut()
            .find(|spec| spec["specId"] == "d2ffa3b7-6c55-5cdf-8512-229d92791333")
            .unwrap();
        search_spec["compatiblePlacementTypes"] = json!(["PRODUCT_PAGE_HEADER_ASSET"]);
        assert!(resolve(
            &incompatible,
            "APP_STORE_SEARCH_RESULTS_ASSET",
            1920,
            1280,
            false
        )
        .is_err());

        let mut malformed_ratio = data;
        let specs = malformed_ratio["data"][0]["attributes"]["imageSpecs"]
            .as_array_mut()
            .unwrap();
        let search_spec = specs
            .iter_mut()
            .find(|spec| spec["specId"] == "d2ffa3b7-6c55-5cdf-8512-229d92791333")
            .unwrap();
        search_spec["aspectRatio"] = json!("3:x");
        assert!(resolve(
            &malformed_ratio,
            "APP_STORE_SEARCH_RESULTS_ASSET",
            1920,
            1280,
            false
        )
        .is_err());
    }

    #[test]
    fn any_platform_groups_are_allowed_but_other_platforms_are_not() {
        let mut data = catalog();
        data["data"][0]["attributes"]["placementProfileGroups"][0]["platform"] = json!("MAC_OS");
        let error =
            resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 1920, 1280, false).unwrap_err();
        assert_eq!(error.code, "placement_spec_not_found");
    }

    #[test]
    fn multiple_compatible_groups_remain_ambiguous() {
        let mut data = catalog();
        let attrs = &mut data["data"][0]["attributes"];
        attrs["placementProfileGroups"]
            .as_array_mut()
            .unwrap()
            .push(json!({"placementProfileGroupId":"SECOND_PROFILE","platform":"ANY"}));
        attrs["features"][0]["placementPolicies"][1]["groupLimits"][0]["groupIds"]
            .as_array_mut()
            .unwrap()
            .push(json!("SECOND_PROFILE"));
        attrs["placementTypes"][1]["specMappings"]
            .as_array_mut()
            .unwrap()
            .push(json!({"placementGroupId":"SECOND_PROFILE","specs":["d2ffa3b7-6c55-5cdf-8512-229d92791333"]}));
        let error =
            resolve(&data, "APP_STORE_SEARCH_RESULTS_ASSET", 1920, 1280, false).unwrap_err();
        assert_eq!(error.code, "placement_ambiguous");
    }

    #[test]
    fn upload_destinations_accept_only_apple_storage_families() {
        use super::upload_destination_allowed;
        use reqwest::Url;

        for value in [
            "https://northamerica-1.object-storage.apple.com/upload/part?signature=synthetic",
            "https://object-storage.apple.com/upload/part?signature=synthetic",
            "https://store-030.blobstore.apple.com/upload/part?signature=synthetic",
            "https://blobstore.apple.com/upload/part?signature=synthetic",
        ] {
            assert!(
                upload_destination_allowed(&Url::parse(value).unwrap()),
                "{value}"
            );
        }
        for value in [
            "https://object-storage.apple.com.evil.invalid/upload",
            "https://evilobject-storage.apple.com/upload",
            "https://object-storage.apple.co/upload",
            "https://example.invalid/upload",
            "http://northamerica-1.object-storage.apple.com/upload",
            "https://northamerica-1.object-storage.apple.com:444/upload",
            "https://user@northamerica-1.object-storage.apple.com/upload",
            "https://user:pass@northamerica-1.object-storage.apple.com/upload",
        ] {
            assert!(
                !upload_destination_allowed(&Url::parse(value).unwrap()),
                "{value}"
            );
        }
    }
}
