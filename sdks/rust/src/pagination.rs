use crate::{auth::now, Client, Error, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::Url;
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub binding: String,
    pub next: String,
    pub seen: Vec<String>,
    pub expires: u64,
}

pub(crate) fn hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(&digest::digest(&digest::SHA256, value.as_bytes()).as_ref()[..16])
}

pub(crate) fn apple_url(origin: &Url, path: &str, query: &BTreeMap<String, String>) -> Result<Url> {
    let mut url = origin.join(path).map_err(|_| Error::invalid_upstream())?;
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

pub(crate) fn next_page(payload: &serde_json::Value, initial: &Url) -> Result<Option<Url>> {
    let Some(link) = payload
        .get("links")
        .and_then(|v| v.get("next"))
        .filter(|v| !v.is_null())
    else {
        return Ok(None);
    };
    let link = link
        .as_str()
        .filter(|v| v.len() <= 8192)
        .ok_or_else(Error::invalid_upstream)?;
    let url = initial.join(link).map_err(|_| Error::invalid_upstream())?;
    if url.origin() != initial.origin()
        || url.path() != initial.path()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::invalid_upstream());
    }
    let initial_query: BTreeMap<_, _> = initial
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut keys = HashSet::new();
    let mut cursor = None;
    for (name, value) in url.query_pairs() {
        if !keys.insert(name.to_string()) {
            return Err(Error::invalid_upstream());
        }
        if name == "cursor" {
            cursor = Some(value.into_owned());
        } else if initial_query.get(name.as_ref()).map(String::as_str) != Some(value.as_ref()) {
            return Err(Error::invalid_upstream());
        }
    }
    let cursor = cursor
        .filter(|v| !v.is_empty() && v.len() <= 2048)
        .ok_or_else(Error::invalid_upstream)?;
    let mut result = initial.clone();
    result.query_pairs_mut().append_pair("cursor", &cursor);
    Ok(Some(result))
}

impl Client {
    pub(crate) fn encode_cursor(&self, cursor: &Cursor) -> Result<String> {
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(cursor).map_err(|_| Error::invalid_upstream())?);
        let signature =
            URL_SAFE_NO_PAD.encode(hmac::sign(&self.inner.cursor_key, payload.as_bytes()));
        let token = format!("{payload}.{signature}");
        if token.len() > 8192 {
            return Err(Error::invalid_upstream());
        }
        Ok(token)
    }

    pub(crate) fn decode_cursor(&self, token: &str, binding: &str) -> Result<Cursor> {
        let invalid = || {
            Error::new(
                400,
                "invalid_cursor",
                "The cursor is invalid, expired, or belongs to another client or query.",
            )
        };
        if token.len() > 8192 {
            return Err(invalid());
        }
        let (payload, signature) = token.split_once('.').ok_or_else(invalid)?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
        hmac::verify(&self.inner.cursor_key, payload.as_bytes(), &signature)
            .map_err(|_| invalid())?;
        let cursor: Cursor =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        if cursor.binding != binding
            || cursor.expires <= now()
            || cursor.expires > now() + 900
            || cursor.next.is_empty()
            || cursor.next.len() > 2048
            || cursor.seen.is_empty()
            || cursor.seen.len() > 100
            || cursor.seen.iter().collect::<HashSet<_>>().len() != cursor.seen.len()
        {
            return Err(invalid());
        }
        Ok(cursor)
    }
}
