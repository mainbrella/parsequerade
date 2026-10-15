use crate::{Error, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::json;
use std::{
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn credentials_error() -> Error {
    Error::new(
        400,
        "invalid_credentials",
        "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.",
    )
}

pub(crate) struct Auth {
    issuer_id: String,
    key_id: String,
    key: EcdsaKeyPair,
    token: Mutex<Option<(String, u64)>>,
}

impl Auth {
    pub(crate) fn new(issuer_id: &str, key_id: &str, pem: &str) -> Result<Self> {
        if issuer_id.is_empty() || issuer_id.len() > 128 || key_id.is_empty() || key_id.len() > 128
        {
            return Err(credentials_error());
        }
        let body = pem
            .trim()
            .strip_prefix("-----BEGIN PRIVATE KEY-----")
            .and_then(|v| v.strip_suffix("-----END PRIVATE KEY-----"))
            .ok_or_else(credentials_error)?;
        let der = STANDARD
            .decode(body.split_whitespace().collect::<String>())
            .map_err(|_| credentials_error())?;
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &SystemRandom::new())
                .map_err(|_| credentials_error())?;
        Ok(Self {
            issuer_id: issuer_id.into(),
            key_id: key_id.into(),
            key,
            token: Mutex::new(None),
        })
    }

    pub(crate) fn token(&self) -> Result<String> {
        let mut cache = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((jwt, expires)) = cache.as_ref() {
            if *expires > now() + 60 {
                return Ok(jwt.clone());
            }
        }
        let seconds = now();
        let expires = seconds + 600;
        let header = URL_SAFE_NO_PAD
            .encode(json!({"alg":"ES256", "kid":self.key_id, "typ":"JWT"}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(json!({"iss":self.issuer_id, "iat":seconds.saturating_sub(30), "exp":expires, "aud":"appstoreconnect-v1"}).to_string());
        let content = format!("{header}.{payload}");
        let signature = self
            .key
            .sign(&SystemRandom::new(), content.as_bytes())
            .map_err(|_| credentials_error())?;
        let jwt = format!("{content}.{}", URL_SAFE_NO_PAD.encode(signature));
        *cache = Some((jwt.clone(), expires));
        Ok(jwt)
    }

    pub(crate) fn invalidate(&self, rejected: &str) {
        let mut cache = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if cache.as_ref().is_some_and(|(jwt, _)| jwt == rejected) {
            *cache = None;
        }
    }
}
