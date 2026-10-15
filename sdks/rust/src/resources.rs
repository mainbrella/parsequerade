use crate::{
    auth::now,
    contract::{self, check_parent, flatten, resource, Route},
    pagination::{apple_url, hash, next_page, Cursor},
    transport::Budget,
    Client, Error, Query, Result,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

fn page_data<'a>(payload: &'a Value, kind: &str, limit: usize) -> Result<&'a Vec<Value>> {
    let items = payload["data"]
        .as_array()
        .filter(|v| v.len() <= limit)
        .ok_or_else(Error::invalid_upstream)?;
    if !payload["links"].is_object() {
        return Err(Error::invalid_upstream());
    }
    let mut ids = HashSet::new();
    for item in items {
        resource(item, kind)?;
        if !ids.insert(item["id"].as_str().unwrap()) {
            return Err(Error::invalid_upstream());
        }
    }
    Ok(items)
}

impl Client {
    async fn traverse<F>(
        &self,
        path: &str,
        query: BTreeMap<String, String>,
        route: &Route,
        budget: &mut Budget,
        mut visit: F,
    ) -> Result<()>
    where
        F: FnMut(&[Value]) -> Result<bool>,
    {
        let initial = apple_url(&self.inner.origin, path, &query)?;
        let mut url = initial.clone();
        let mut seen = HashSet::new();
        for _ in 0..20 {
            if !seen.insert(hash(url.as_str())) {
                return Err(Error::incomplete());
            }
            let payload = self.apple(url, None, budget, route).await?;
            let items = page_data(&payload, &route.resource_type, 200)?;
            if visit(items)? {
                return Ok(());
            }
            match next_page(&payload, &initial)? {
                Some(next) => url = next,
                None => return Ok(()),
            }
        }
        Err(Error::incomplete())
    }

    pub(crate) async fn read(
        &self,
        path: &str,
        query: &Query,
        budget: &mut Budget,
    ) -> Result<Value> {
        let (route, p) = contract::match_route(path)?;
        let mut query = contract::query_for(route, query)?;
        let handler = route.handler.as_str();
        let info = handler.starts_with("appInfo") || handler == "ageRating";
        let app_id = p.get("appId").map(String::as_str).unwrap_or("");
        let parent_id = p
            .get(if info { "appInfoId" } else { "versionId" })
            .map(String::as_str);
        let parent_type = if info { "appInfos" } else { "appStoreVersions" };
        let mut context = Map::new();
        if !app_id.is_empty() && !matches!(handler, "apps" | "app") {
            context.insert("appId".into(), app_id.into());
            if let Some(id) = parent_id.filter(|_| !matches!(handler, "appInfo" | "version")) {
                context.insert(
                    if info { "appInfoId" } else { "versionId" }.into(),
                    id.into(),
                );
            }
        }
        if let Some(parent_id) = parent_id {
            let mut found = false;
            let mut parameters = BTreeMap::from([("limit".into(), "200".into())]);
            if !info {
                parameters.insert("filter[id]".into(), parent_id.into());
            }
            self.traverse(
                &format!("/v1/apps/{app_id}/{parent_type}"),
                parameters,
                contract::route(if info { "appInfos" } else { "versions" }),
                budget,
                |items| {
                    for item in items {
                        check_parent(item, "app", app_id, "apps")?;
                    }
                    found = items
                        .iter()
                        .any(|item| item["id"].as_str() == Some(parent_id));
                    Ok(found)
                },
            )
            .await?;
            if !found {
                return Err(Error::not_found("resource_not_found"));
            }
        }
        let id = parent_id.unwrap_or("");
        let mut localization_id = None;
        let apple_path = match handler {
            "apps" => "/v1/apps".into(),
            "app" => format!("/v1/apps/{app_id}"),
            "appInfos" => format!("/v1/apps/{app_id}/appInfos"),
            "appInfo" => format!("/v1/appInfos/{id}"),
            "versions" => format!("/v1/apps/{app_id}/appStoreVersions"),
            "version" => format!("/v1/appStoreVersions/{id}"),
            "appInfoLocalizations" => format!("/v1/appInfos/{id}/appInfoLocalizations"),
            "versionLocalizations" => {
                format!("/v1/appStoreVersions/{id}/appStoreVersionLocalizations")
            }
            "ageRating" => format!("/v1/appInfos/{id}/ageRatingDeclaration"),
            "reviewContact" => format!("/v1/appStoreVersions/{id}/appStoreReviewDetail"),
            "screenshots" => format!(
                "/v1/appScreenshotSets/{}/appScreenshots",
                p["screenshotSetId"]
            ),
            "previews" => format!("/v1/appPreviewSets/{}/appPreviews", p["previewSetId"]),
            "appInfoLocalization" | "versionLocalization" | "screenshotSets" | "previewSets" => {
                let localizations = if info {
                    "appInfoLocalizations"
                } else {
                    "appStoreVersionLocalizations"
                };
                let locale = &p["locale"];
                let parameters = BTreeMap::from([
                    ("limit".into(), "200".into()),
                    ("filter[locale]".into(), locale.clone()),
                    (format!("fields[{localizations}]"), "locale".into()),
                ]);
                let mut selected = None;
                self.traverse(
                    &format!("/v1/{parent_type}/{id}/{localizations}"),
                    parameters,
                    contract::route(if info {
                        "appInfoLocalizations"
                    } else {
                        "versionLocalizations"
                    }),
                    budget,
                    |items| {
                        for item in items {
                            check_parent(
                                item,
                                if info { "appInfo" } else { "appStoreVersion" },
                                id,
                                parent_type,
                            )?;
                            let item_locale = item["attributes"]["locale"]
                                .as_str()
                                .ok_or_else(Error::invalid_upstream)?;
                            if item_locale != locale {
                                continue;
                            }
                            if selected.is_some() {
                                return Err(Error::new(
                                    409,
                                    "ambiguous_selection",
                                    "More than one localization matches this locale.",
                                ));
                            }
                            selected = Some(item["id"].as_str().unwrap().to_owned());
                        }
                        Ok(false)
                    },
                )
                .await?;
                let selected =
                    selected.ok_or_else(|| Error::not_found("localization_not_found"))?;
                if matches!(handler, "screenshotSets" | "previewSets") {
                    let sets = if handler == "screenshotSets" {
                        "appScreenshotSets"
                    } else {
                        "appPreviewSets"
                    };
                    localization_id = Some(selected.clone());
                    format!("/v1/{localizations}/{selected}/{sets}")
                } else {
                    if let Some(fields) = &query.fields {
                        if !fields.iter().any(|f| f == "locale") {
                            query.upstream.insert(
                                format!("fields[{localizations}]"),
                                format!("{},locale", fields.join(",")),
                            );
                        }
                    }
                    format!("/v1/{localizations}/{selected}")
                }
            }
            _ => return Err(Error::not_found("route_not_found")),
        };
        if matches!(handler, "appInfos" | "appInfo") {
            query
                .upstream
                .insert("include".into(), "primaryCategory,secondaryCategory".into());
            if let Some(fields) = &query.fields {
                query.upstream.insert(
                    "fields[appInfos]".into(),
                    format!("{},primaryCategory,secondaryCategory", fields.join(",")),
                );
            }
        }
        let initial = apple_url(&self.inner.origin, &apple_path, &query.upstream)?;
        if !route.collection {
            let payload = self.apple(initial, None, budget, route).await?;
            let data = &payload["data"];
            if data.is_null() {
                return Err(Error::not_found(if handler == "reviewContact" {
                    "review_contact_not_found"
                } else {
                    "resource_not_found"
                }));
            }
            resource(data, &route.resource_type)?;
            let expected = if handler == "app" {
                Some(app_id)
            } else if matches!(handler, "appInfo" | "version") {
                parent_id
            } else if p.contains_key("locale") {
                apple_path.rsplit('/').next()
            } else {
                None
            };
            if expected.is_some_and(|id| data["id"].as_str() != Some(id)) {
                return Err(Error::invalid_upstream());
            }
            if matches!(handler, "appInfo" | "version") {
                check_parent(data, "app", app_id, "apps")?;
            }
            if let Some(locale) = p.get("locale") {
                if data["attributes"]["locale"].as_str() != Some(locale) {
                    return Err(Error::invalid_upstream());
                }
                check_parent(
                    data,
                    if info { "appInfo" } else { "appStoreVersion" },
                    id,
                    parent_type,
                )?;
            }
            if handler == "reviewContact" {
                check_parent(data, "appStoreVersion", id, parent_type)?;
            }
            return flatten(data, route, &context, query.fields.as_deref());
        }
        let binding = hash(&format!("{path}\n{initial}"));
        let prior = query
            .cursor
            .as_deref()
            .map(|c| self.decode_cursor(c, &binding))
            .transpose()?;
        let expires = prior.as_ref().map_or(now() + 900, |c| c.expires);
        let mut seen = prior.as_ref().map_or_else(Vec::new, |c| c.seen.clone());
        if seen.len() >= 100 {
            return Err(Error::incomplete());
        }
        let mut url = initial.clone();
        if let Some(prior) = prior {
            url.query_pairs_mut().append_pair("cursor", &prior.next);
        }
        let page_hash = hash(url.as_str());
        if seen.contains(&page_hash) {
            return Err(Error::incomplete());
        }
        seen.push(page_hash);
        let payload = self.apple(url, None, budget, route).await?;
        let mut items = Vec::new();
        for item in page_data(&payload, &route.resource_type, query.limit)? {
            if matches!(handler, "appInfos" | "versions") {
                check_parent(item, "app", app_id, "apps")?;
            }
            if matches!(handler, "appInfoLocalizations" | "versionLocalizations") {
                check_parent(
                    item,
                    if info { "appInfo" } else { "appStoreVersion" },
                    id,
                    parent_type,
                )?;
            }
            if let Some(localization_id) = &localization_id {
                check_parent(
                    item,
                    "appStoreVersionLocalization",
                    localization_id,
                    "appStoreVersionLocalizations",
                )?;
            }
            if handler == "screenshots" {
                check_parent(
                    item,
                    "appScreenshotSet",
                    &p["screenshotSetId"],
                    "appScreenshotSets",
                )?;
            }
            if handler == "previews" {
                check_parent(item, "appPreviewSet", &p["previewSetId"], "appPreviewSets")?;
            }
            items.push(flatten(item, route, &context, query.fields.as_deref())?);
        }
        let next_cursor = if let Some(next) = next_page(&payload, &initial)? {
            if seen.contains(&hash(next.as_str())) {
                return Err(Error::incomplete());
            }
            let next = next
                .query_pairs()
                .find(|(k, _)| k == "cursor")
                .unwrap()
                .1
                .into_owned();
            Some(self.encode_cursor(&Cursor {
                binding,
                next,
                seen,
                expires,
            })?)
        } else {
            None
        };
        Ok(json!({"items":items, "nextCursor":next_cursor}))
    }
}
