use crate::{Error, Query, Result};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Contracts {
    pub routes: Vec<Route>,
    pub age_rating_write: BTreeMap<String, Schema>,
}
#[derive(Deserialize)]
pub(crate) struct Route {
    pub handler: String,
    pub route: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    pub collection: bool,
    pub attributes: BTreeMap<String, Schema>,
    pub relationships: BTreeMap<String, Relationship>,
    pub parameters: Vec<Parameter>,
}
#[derive(Deserialize)]
pub(crate) struct Schema {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(rename = "enum")]
    pub choices: Option<Vec<String>>,
    pub items: Option<Box<Schema>>,
    pub maximum: Option<u64>,
    pub format: Option<String>,
}
#[derive(Deserialize)]
pub(crate) struct Relationship {
    pub field: String,
    #[serde(rename = "type")]
    pub resource_type: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Parameter {
    pub name: String,
    pub schema: Schema,
    pub apple_name: Option<String>,
    pub default: Option<Value>,
}

pub(crate) fn contracts() -> &'static Contracts {
    static CONTRACTS: OnceLock<Contracts> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        serde_json::from_str(include_str!("contracts.json")).expect("generated contracts")
    })
}

pub(crate) fn route(handler: &str) -> &'static Route {
    contracts()
        .routes
        .iter()
        .find(|r| r.handler == handler)
        .expect("implemented handler")
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub(crate) fn validate_id(value: &str) -> Result<()> {
    if valid_id(value) {
        Ok(())
    } else {
        Err(Error::new(
            400,
            "invalid_path",
            "The resource path is invalid.",
        ))
    }
}
pub(crate) fn validate_locale(value: &str) -> Result<()> {
    if (2..=35).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        Ok(())
    } else {
        Err(Error::new(400, "invalid_path", "The locale is invalid."))
    }
}

pub(crate) fn match_route(path: &str) -> Result<(&'static Route, BTreeMap<String, String>)> {
    let parts: Vec<_> = path.split('/').collect();
    for route in &contracts().routes {
        let pattern: Vec<_> = route.route.split('/').collect();
        if parts.len() != pattern.len() {
            continue;
        }
        let mut params = BTreeMap::new();
        let mut matched = true;
        for (part, template) in parts.iter().zip(pattern) {
            if template.starts_with('{') {
                let name = &template[1..template.len() - 1];
                if name == "locale" {
                    validate_locale(part)?;
                } else {
                    validate_id(part)?;
                }
                params.insert(name.into(), (*part).into());
            } else if *part != template {
                matched = false;
                break;
            }
        }
        if matched {
            return Ok((route, params));
        }
    }
    Err(Error::not_found("route_not_found"))
}

pub(crate) struct ParsedQuery {
    pub upstream: BTreeMap<String, String>,
    pub fields: Option<Vec<String>>,
    pub cursor: Option<String>,
    pub limit: usize,
}
pub(crate) fn query_for(route: &Route, query: &Query) -> Result<ParsedQuery> {
    let mut result = ParsedQuery {
        upstream: BTreeMap::new(),
        fields: None,
        cursor: None,
        limit: 50,
    };
    let mut seen = std::collections::HashSet::new();
    for (name, raw) in &query.0 {
        let parameter = route
            .parameters
            .iter()
            .find(|p| &p.name == name)
            .ok_or_else(|| Error::invalid_query(name))?;
        if !seen.insert(name)
            || raw.is_empty()
            || raw.len() > if name == "cursor" { 8192 } else { 4096 }
        {
            return Err(Error::invalid_query(name));
        }
        let schema = &parameter.schema;
        let valid = match schema.kind.as_str() {
            "integer" => {
                raw.bytes().all(|b| b.is_ascii_digit())
                    && !raw.starts_with('0')
                    && raw.len() <= 3
                    && raw
                        .parse::<u64>()
                        .is_ok_and(|n| n > 0 && n <= schema.maximum.unwrap_or(200))
            }
            "boolean" => raw == "true" || raw == "false",
            "array" => {
                let values: Vec<_> = raw.split(',').collect();
                values.len() <= 200
                    && values.iter().all(|v| {
                        !v.is_empty()
                            && v.len() <= 256
                            && schema
                                .items
                                .as_ref()
                                .and_then(|s| s.choices.as_ref())
                                .is_none_or(|choices| choices.iter().any(|c| c == v))
                    })
            }
            _ => true,
        };
        if !valid {
            return Err(Error::invalid_query(name));
        }
        match name.as_str() {
            "fields" => result.fields = Some(raw.split(',').map(str::to_owned).collect()),
            "cursor" => result.cursor = Some(raw.clone()),
            "limit" => result.limit = raw.parse().map_err(|_| Error::invalid_query(name))?,
            _ => {}
        }
        if let Some(apple_name) = &parameter.apple_name {
            result.upstream.insert(apple_name.clone(), raw.clone());
        }
    }
    for parameter in &route.parameters {
        if !seen.contains(&parameter.name) {
            if let (Some(default), Some(apple_name)) = (&parameter.default, &parameter.apple_name) {
                result
                    .upstream
                    .insert(apple_name.clone(), default.to_string());
            }
        }
    }
    Ok(result)
}

pub(crate) fn resource<'a>(value: &'a Value, kind: &str) -> Result<&'a Value> {
    if !value.is_object()
        || value["type"].as_str() != Some(kind)
        || !value["id"].as_str().is_some_and(valid_id)
        || value.get("attributes").is_some_and(|a| !a.is_object())
        || value.get("relationships").is_some_and(|r| !r.is_object())
    {
        return Err(Error::invalid_upstream());
    }
    Ok(value)
}
pub(crate) fn check_parent(value: &Value, relation: &str, id: &str, kind: &str) -> Result<()> {
    if let Some(data) = value
        .get("relationships")
        .and_then(|v| v.get(relation))
        .and_then(|v| v.get("data"))
    {
        if !data.is_object()
            || data["id"].as_str() != Some(id)
            || data["type"].as_str() != Some(kind)
        {
            return Err(Error::invalid_upstream());
        }
    }
    Ok(())
}
pub(crate) fn flatten(
    value: &Value,
    route: &Route,
    context: &Map<String, Value>,
    fields: Option<&[String]>,
) -> Result<Value> {
    resource(value, &route.resource_type)?;
    let mut result = context.clone();
    result.insert("id".into(), value["id"].clone());
    for (name, schema) in &route.attributes {
        if fields.is_some_and(|f| !f.contains(name)) {
            continue;
        }
        if let Some(item) = value.get("attributes").and_then(|a| a.get(name)) {
            if !item.is_null()
                && !matches!(
                    (schema.kind.as_str(), item),
                    ("string", Value::String(_)) | ("boolean", Value::Bool(_))
                )
            {
                return Err(Error::invalid_upstream());
            }
            result.insert(name.clone(), item.clone());
        }
    }
    for (name, relation) in &route.relationships {
        if let Some(data) = value
            .get("relationships")
            .and_then(|r| r.get(name))
            .and_then(|r| r.get("data"))
        {
            if !data.is_null()
                && (!data.is_object()
                    || data["type"].as_str() != Some(&relation.resource_type)
                    || !data["id"].as_str().is_some_and(valid_id))
            {
                return Err(Error::invalid_upstream());
            }
            result.insert(
                relation.field.clone(),
                if data.is_null() {
                    Value::Null
                } else {
                    data["id"].clone()
                },
            );
        }
    }
    Ok(Value::Object(result))
}

pub(crate) fn validate_update(value: &Value) -> Result<()> {
    let invalid = || {
        Error::new(
            400,
            "invalid_request",
            "Provide a nonempty object of supported age rating attributes.",
        )
    };
    let attrs = value
        .as_object()
        .filter(|v| !v.is_empty())
        .ok_or_else(invalid)?;
    if value.to_string().len() > 8192 {
        return Err(Error::new(
            413,
            "request_too_large",
            "Age rating attributes must fit within 8 KiB.",
        ));
    }
    for (name, value) in attrs {
        let invalid_field = || {
            let mut error = invalid();
            error.details.push(crate::ErrorDetail {
                code: "invalid_attribute".into(),
                message: "This age rating attribute or value is unsupported.".into(),
                field: Some(name.clone()),
            });
            error
        };
        let schema = contracts()
            .age_rating_write
            .get(name)
            .ok_or_else(invalid_field)?;
        if value.is_null() {
            if schema.nullable {
                continue;
            } else {
                return Err(invalid_field());
            }
        }
        if !matches!(
            (schema.kind.as_str(), value),
            ("boolean", Value::Bool(_)) | ("string", Value::String(_))
        ) {
            return Err(invalid_field());
        }
        if let Some(text) = value.as_str() {
            if text.encode_utf16().count() > 2048
                || schema
                    .choices
                    .as_ref()
                    .is_some_and(|c| !c.iter().any(|v| v == text))
            {
                return Err(invalid_field());
            }
            if schema.format.as_deref() == Some("uri") {
                let url = Url::parse(text).map_err(|_| invalid_field())?;
                if !["http", "https"].contains(&url.scheme())
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(invalid_field());
                }
            }
        }
    }
    Ok(())
}
