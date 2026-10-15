use super::*;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ring::signature::{self, KeyPair};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

pub(super) struct Step {
    method: &'static str,
    path: String,
    query: BTreeMap<String, String>,
    status: u16,
    payload: Value,
    headers: Vec<(&'static str, String)>,
    body: Option<Value>,
    disconnect: bool,
    delay: Duration,
}
impl Step {
    pub(super) fn get(path: &str, payload: Value) -> Self {
        Self {
            method: "GET",
            path: path.into(),
            query: BTreeMap::new(),
            status: 200,
            payload,
            headers: Vec::new(),
            body: None,
            disconnect: false,
            delay: Duration::ZERO,
        }
    }
    fn query(mut self, pairs: &[(&str, &str)]) -> Self {
        self.query = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        self
    }
    fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
    fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.into()));
        self
    }
    pub(super) fn delay(mut self, duration: Duration) -> Self {
        self.delay = duration;
        self
    }
    pub(super) fn patch(mut self, body: Value) -> Self {
        self.method = "PATCH";
        self.body = Some(body);
        self
    }
}
fn data(kind: &str, id: &str, attributes: Value) -> Value {
    json!({"type":kind, "id":id, "attributes":attributes})
}
fn page(items: Vec<Value>, next: Option<&str>) -> Value {
    json!({"data":items, "links":{"next":next}})
}
fn single(kind: &str, id: &str, attributes: Value) -> Value {
    json!({"data":data(kind, id, attributes)})
}
fn pem() -> (String, Vec<u8>) {
    let der = signature::EcdsaKeyPair::generate_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        &SystemRandom::new(),
    )
    .unwrap();
    let key = signature::EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        der.as_ref(),
        &SystemRandom::new(),
    )
    .unwrap();
    (
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            STANDARD.encode(der)
        ),
        key.public_key().as_ref().to_vec(),
    )
}

pub(super) async fn mock(steps: Vec<Step>) -> (Client, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (pem, public_key) = pem();
    let mut client = Client::new("issuer-fixture", "KEY123", &pem).unwrap();
    let inner = Arc::get_mut(&mut client.inner).unwrap();
    inner.origin = Url::parse(&format!("http://{address}")).unwrap();
    inner.http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let task = tokio::spawn(async move {
        let mut tokens = Vec::new();
        for step in steps {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("expected Apple request")
                .unwrap();
            let mut bytes = Vec::new();
            let (header_end, content_length) = loop {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]);
                    let len = head
                        .lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map_or(0, |(_, v)| v.trim().parse::<usize>().unwrap());
                    break (end + 4, len);
                }
            };
            while bytes.len() < header_end + content_length {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            let head = String::from_utf8_lossy(&bytes[..header_end]);
            let mut lines = head.lines();
            let first: Vec<_> = lines.next().unwrap().split_whitespace().collect();
            assert_eq!(first[0], step.method);
            let url = Url::parse(&format!("http://{address}{}", first[1])).unwrap();
            assert_eq!(url.path(), step.path);
            let actual: BTreeMap<_, _> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            assert_eq!(actual, step.query, "query for {}", step.path);
            let headers: BTreeMap<_, _> = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
                .collect();
            let token = headers["authorization"].strip_prefix("Bearer ").unwrap();
            let (content, sig) = token.rsplit_once('.').unwrap();
            signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, &public_key)
                .verify(content.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).unwrap())
                .unwrap();
            let segments: Vec<_> = token.split('.').collect();
            let jwt_header: Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segments[0]).unwrap()).unwrap();
            let claims: Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segments[1]).unwrap()).unwrap();
            assert_eq!(
                jwt_header,
                json!({"alg":"ES256", "kid":"KEY123", "typ":"JWT"})
            );
            assert_eq!(claims["aud"], "appstoreconnect-v1");
            assert_eq!(claims["iss"], "issuer-fixture");
            assert_eq!(
                claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
                630
            );
            tokens.push(token.to_owned());
            if let Some(body) = step.body {
                assert_eq!(headers["content-type"], "application/json");
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes[header_end..]).unwrap(),
                    body
                );
            } else {
                assert_eq!(content_length, 0);
            }
            if step.disconnect {
                continue;
            }
            tokio::time::sleep(step.delay).await;
            let body = step.payload.to_string();
            let mut response = format!(
                "HTTP/1.1 {} Mock\r\nConnection: close\r\nContent-Length: {}\r\n",
                step.status,
                body.len()
            );
            if !step
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                response.push_str("Content-Type: application/json\r\n");
            }
            for (name, value) in step.headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&body);
            // A bounded client may close before receiving a delayed or oversized body.
            let _ = stream.write_all(response.as_bytes()).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(350), listener.accept())
                .await
                .is_err(),
            "unexpected extra Apple request"
        );
        tokens
    });
    (client, task)
}
fn app_info_parent() -> Step {
    Step::get(
        "/v1/apps/app1/appInfos",
        page(
            vec![data(
                "appInfos",
                "info1",
                json!({"state":"PREPARE_FOR_SUBMISSION"}),
            )],
            None,
        ),
    )
    .query(&[("limit", "200")])
}
fn version_parent() -> Step {
    Step::get(
        "/v1/apps/app1/appStoreVersions",
        page(vec![data("appStoreVersions", "version1", json!({}))], None),
    )
    .query(&[("limit", "200"), ("filter[id]", "version1")])
}

#[tokio::test]
async fn flat_apps_query_translation_pagination_and_jwt_reuse() {
    let (client, task) = mock(vec![
        Step::get("/v1/apps", page(vec![data("apps", "app1", json!({"name":"Notes", "bundleId":"com.example.notes", "unknown":"hidden", "streamlinedPurchasingEnabled":false}))], Some("?cursor=page2")))
            .query(&[("filter[bundleId]", "com.example.notes"), ("limit", "1")]),
        Step::get("/v1/apps", page(vec![data("apps", "app2", json!({"name":"Other"}))], None))
            .query(&[("filter[bundleId]", "com.example.notes"), ("limit", "1"), ("cursor", "page2")]),
    ]).await;
    let q = Query::new()
        .with("bundleId", "com.example.notes")
        .with("limit", 1);
    let first = client.apps(&q).await.unwrap();
    assert_eq!(first.items[0].name.as_deref(), Some("Notes"));
    assert_eq!(first.items[0].streamlined_purchasing_enabled, Some(false));
    let token = first.next_cursor.unwrap();
    let error = client
        .apps(&Query::new().with("limit", 1).with("cursor", &token))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_cursor");
    let second = client.clone().apps(&q.with("cursor", token)).await.unwrap();
    assert_eq!(second.items[0].id, "app2");
    assert!(second.next_cursor.is_none());
    let tokens = task.await.unwrap();
    assert_eq!(tokens[0], tokens[1]);
}

#[tokio::test]
async fn all_remaining_read_routes_and_relationships() {
    let (client, task) = mock(vec![
        Step::get("/v1/apps/app1", single("apps", "app1", json!({"name":"Notes", "sku":null}))).query(&[("fields[apps]", "name,sku")]),
        Step::get("/v1/apps/app1/appInfos", json!({"data":[{"type":"appInfos", "id":"info1", "attributes":{"state":"LIVE"}, "relationships":{"primaryCategory":{"data":{"type":"appCategories", "id":"UTILITIES"}}, "secondaryCategory":{"data":null}}}], "links":{}}))
            .query(&[("limit", "50"), ("include", "primaryCategory,secondaryCategory")]),
        app_info_parent(),
        Step::get("/v1/appInfos/info1", single("appInfos", "info1", json!({"state":"LIVE"}))).query(&[("fields[appInfos]", "state,primaryCategory,secondaryCategory"), ("include", "primaryCategory,secondaryCategory")]),
        app_info_parent(),
        Step::get("/v1/appInfos/info1/appInfoLocalizations", page(vec![data("appInfoLocalizations", "loc1", json!({"locale":"en-US", "name":"Notes"}))], None)).query(&[("limit", "50"), ("filter[locale]", "en-US")]),
        app_info_parent(),
        Step::get("/v1/appInfos/info1/appInfoLocalizations", page(vec![data("appInfoLocalizations", "loc1", json!({"locale":"en-US"}))], None)).query(&[("limit", "200"), ("filter[locale]", "en-US"), ("fields[appInfoLocalizations]", "locale")]),
        Step::get("/v1/appInfoLocalizations/loc1", single("appInfoLocalizations", "loc1", json!({"locale":"en-US", "name":"Notes", "privacyPolicyText":"private"}))).query(&[("fields[appInfoLocalizations]", "name,locale")]),
        app_info_parent(),
        Step::get("/v1/appInfos/info1/ageRatingDeclaration", single("ageRatingDeclarations", "rating1", json!({"gambling":false}))),
        Step::get("/v1/apps/app1/appStoreVersions", page(vec![data("appStoreVersions", "version1", json!({"platform":"IOS", "versionString":"1.2"}))], None)).query(&[("limit", "50"), ("filter[platform]", "IOS"), ("filter[appVersionState]", "PREPARE_FOR_SUBMISSION")]),
        version_parent(),
        Step::get("/v1/appStoreVersions/version1", single("appStoreVersions", "version1", json!({"versionString":"1.2"}))),
        version_parent(),
        Step::get("/v1/appStoreVersions/version1/appStoreVersionLocalizations", page(vec![data("appStoreVersionLocalizations", "vloc1", json!({"locale":"fr-FR", "description":"Bonjour"}))], None)).query(&[("limit", "50")]),
        version_parent(),
        Step::get("/v1/appStoreVersions/version1/appStoreVersionLocalizations", page(vec![data("appStoreVersionLocalizations", "vloc1", json!({"locale":"fr-FR"}))], None)).query(&[("limit", "200"), ("filter[locale]", "fr-FR"), ("fields[appStoreVersionLocalizations]", "locale")]),
        Step::get("/v1/appStoreVersionLocalizations/vloc1", single("appStoreVersionLocalizations", "vloc1", json!({"locale":"fr-FR", "description":"Bonjour"}))),
        version_parent(),
        Step::get("/v1/appStoreVersions/version1/appStoreReviewDetail", single("appStoreReviewDetails", "review1", json!({"contactFirstName":"Alex"}))),
    ]).await;
    let app: Value = client
        .get("/apps/app1", &Query::new().with("fields", "name,sku"))
        .await
        .unwrap();
    assert_eq!(app, json!({"id":"app1", "name":"Notes", "sku":null}));
    let infos = client.app_infos("app1", &Query::new()).await.unwrap();
    assert_eq!(
        infos.items[0].primary_category_id.as_deref(),
        Some("UTILITIES")
    );
    assert_eq!(infos.items[0].app_id, "app1");
    assert_eq!(
        client
            .app_info("app1", "info1", &Query::new().with("fields", "state"))
            .await
            .unwrap()
            .id,
        "info1"
    );
    assert_eq!(
        client
            .app_info_localizations("app1", "info1", &Query::new().with("locale", "en-US"))
            .await
            .unwrap()
            .items[0]
            .app_info_id,
        "info1"
    );
    let loc = client
        .app_info_localization(
            "app1",
            "info1",
            "en-US",
            &Query::new().with("fields", "name"),
        )
        .await
        .unwrap();
    assert_eq!(loc.name.as_deref(), Some("Notes"));
    assert_eq!(loc.locale, None);
    assert_eq!(loc.privacy_policy_text, None);
    assert_eq!(
        client
            .age_rating("app1", "info1", &Query::new())
            .await
            .unwrap()
            .gambling,
        Some(false)
    );
    assert_eq!(
        client
            .versions(
                "app1",
                &Query::new()
                    .with("platform", "IOS")
                    .with("state", "PREPARE_FOR_SUBMISSION")
            )
            .await
            .unwrap()
            .items[0]
            .version_string
            .as_deref(),
        Some("1.2")
    );
    assert_eq!(
        client
            .version("app1", "version1", &Query::new())
            .await
            .unwrap()
            .app_id,
        "app1"
    );
    assert_eq!(
        client
            .version_localizations("app1", "version1", &Query::new())
            .await
            .unwrap()
            .items[0]
            .version_id,
        "version1"
    );
    assert_eq!(
        client
            .version_localization("app1", "version1", "fr-FR", &Query::new())
            .await
            .unwrap()
            .description
            .as_deref(),
        Some("Bonjour")
    );
    assert_eq!(
        client
            .review_contact("app1", "version1", &Query::new())
            .await
            .unwrap()
            .contact_first_name
            .as_deref(),
        Some("Alex")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn writes_resolve_scope_preserve_false_and_null_and_do_not_retry() {
    for status in [200, 401, 429, 503] {
        let attrs =
            json!({"gambling":false, "kidsAgeBand":null, "gracRatingClassificationNumber":""});
        let payload = if status == 200 {
            single("ageRatingDeclarations", "rating1", attrs.clone())
        } else {
            json!({"errors":[{"code":"PRIVATE", "detail":"secret-value"}]})
        };
        let (client, task) = mock(vec![
            app_info_parent(),
            Step::get("/v1/appInfos/info1/ageRatingDeclaration", single("ageRatingDeclarations", "rating1", json!({"gambling":true}))),
            Step::get("/v1/ageRatingDeclarations/rating1", payload).status(status).patch(json!({"data":{"type":"ageRatingDeclarations", "id":"rating1", "attributes":attrs}})),
        ]).await;
        let result = client.update_age_rating("app1", "info1", &attrs).await;
        if status == 200 {
            assert_eq!(result.unwrap().gambling, Some(false));
        } else {
            let error = result.unwrap_err();
            assert!(!error.retryable);
            assert!(!format!("{error:?}").contains("secret-value"));
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn uncertain_writes_are_never_replayed() {
    for disconnect in [false, true] {
        let mut patch = Step::get("/v1/ageRatingDeclarations/rating1", json!({"unexpected":"shape"}))
            .patch(json!({"data":{"type":"ageRatingDeclarations", "id":"rating1", "attributes":{"gambling":false}}}));
        patch.disconnect = disconnect;
        let (client, task) = mock(vec![
            app_info_parent(),
            Step::get(
                "/v1/appInfos/info1/ageRatingDeclaration",
                single("ageRatingDeclarations", "rating1", json!({})),
            ),
            patch,
        ])
        .await;
        let error = client
            .update_age_rating("app1", "info1", &json!({"gambling":false}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "write_outcome_unknown");
        assert!(!error.retryable);
        task.await.unwrap();
    }
}

#[tokio::test]
async fn safe_reads_retry_and_refresh_authentication_once() {
    let steps = vec![
        Step::get("/v1/apps/app1", json!({"errors":[]}))
            .status(503)
            .header("Retry-After", "0"),
        Step::get("/v1/apps/app1", json!({"errors":[]})).status(401),
        Step::get(
            "/v1/apps/app1",
            single("apps", "app1", json!({"name":"Recovered"})),
        ),
    ];
    let (client, task) = mock(steps).await;
    assert_eq!(
        client
            .app("app1", &Query::new())
            .await
            .unwrap()
            .name
            .as_deref(),
        Some("Recovered")
    );
    let tokens = task.await.unwrap();
    assert_eq!(tokens[0], tokens[1]);
    assert_ne!(tokens[1], tokens[2]);
    let (client, task) = mock(vec![
        Step::get("/v1/apps/app1", json!({})).status(401),
        Step::get("/v1/apps/app1", json!({})).status(401),
    ])
    .await;
    let error = client.app("app1", &Query::new()).await.unwrap_err();
    assert_eq!(error.code, "upstream_authentication_failed");
    assert!(!error.retryable);
    task.await.unwrap();
}

#[tokio::test]
async fn long_throttles_are_returned_with_sanitized_field_errors() {
    let (client, task) = mock(vec![Step::get("/v1/apps", json!({"errors":[{"code":"RATE_LIMIT", "detail":"private", "source":{"parameter":"filter[bundleId]"}}]})).query(&[("limit", "50"), ("filter[bundleId]", "com.example")]).status(429).header("Retry-After", "30")]).await;
    let error = client
        .apps(&Query::new().with("bundleId", "com.example"))
        .await
        .unwrap_err();
    assert_eq!(error.retry_after, Some(30));
    assert_eq!(error.details[0].field.as_deref(), Some("bundleId"));
    assert!(!format!("{error:?}").contains("private"));
    task.await.unwrap();
}

#[tokio::test]
async fn ownership_must_be_proven_before_read_or_write() {
    let (client, task) = mock(vec![Step::get(
        "/v1/apps/app1/appInfos",
        page(vec![], None),
    )
    .query(&[("limit", "200")])])
    .await;
    assert_eq!(
        client
            .update_age_rating("app1", "info1", &json!({"gambling":false}))
            .await
            .unwrap_err()
            .code,
        "resource_not_found"
    );
    task.await.unwrap();
}

#[tokio::test]
async fn locale_uniqueness_is_checked_across_pages() {
    let query = [
        ("limit", "200"),
        ("filter[locale]", "en-US"),
        ("fields[appInfoLocalizations]", "locale"),
    ];
    let (client, task) = mock(vec![
        app_info_parent(),
        Step::get(
            "/v1/appInfos/info1/appInfoLocalizations",
            page(
                vec![data(
                    "appInfoLocalizations",
                    "loc1",
                    json!({"locale":"en-US"}),
                )],
                Some("?cursor=second"),
            ),
        )
        .query(&query),
        Step::get(
            "/v1/appInfos/info1/appInfoLocalizations",
            page(
                vec![data(
                    "appInfoLocalizations",
                    "loc2",
                    json!({"locale":"en-US"}),
                )],
                None,
            ),
        )
        .query(&[
            ("limit", "200"),
            ("filter[locale]", "en-US"),
            ("fields[appInfoLocalizations]", "locale"),
            ("cursor", "second"),
        ]),
    ])
    .await;
    assert_eq!(
        client
            .app_info_localization("app1", "info1", "en-US", &Query::new())
            .await
            .unwrap_err()
            .code,
        "ambiguous_selection"
    );
    task.await.unwrap();
}

#[tokio::test]
async fn rejects_malformed_responses_and_untrusted_links() {
    let cases = vec![
        page(vec![], Some("https://attacker.example/v1/apps?cursor=x")),
        page(vec![], Some("/v1/other?cursor=x")),
        page(vec![], Some("?cursor=x&limit=200")),
        page(vec![], Some("?cursor=x&cursor=y")),
        page(vec![], Some("?cursor=x#fragment")),
        page(
            vec![
                data("apps", "same", json!({})),
                data("apps", "same", json!({})),
            ],
            None,
        ),
        page(vec![data("apps", "app1", json!({"name":42}))], None),
        page(vec![data("wrongType", "app1", json!({}))], None),
        json!({"data":[], "links":null}),
    ];
    for payload in cases {
        let (client, task) = mock(vec![
            Step::get("/v1/apps", payload).query(&[("limit", "50")])
        ])
        .await;
        assert_eq!(
            client.apps(&Query::new()).await.unwrap_err().code,
            "upstream_invalid_response"
        );
        task.await.unwrap();
    }
    for step in [
        Step::get("/v1/apps/app1", json!({}))
            .status(302)
            .header("Location", "https://attacker.example/"),
        Step::get("/v1/apps/app1", single("apps", "app1", json!({})))
            .header("Content-Type", "text/html"),
    ] {
        let (client, task) = mock(vec![step]).await;
        assert_eq!(
            client.app("app1", &Query::new()).await.unwrap_err().code,
            "upstream_invalid_response"
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn rejects_invalid_local_input_without_network() {
    let (pem, _) = pem();
    let client = Client::new("issuer", "key", &pem).unwrap();
    assert_eq!(
        client.inner.origin.as_str(),
        "https://api.appstoreconnect.apple.com/"
    );
    assert!(!format!("{client:?}").contains("issuer"));
    for q in [
        Query::new().with("limit", 0),
        Query::new().with("limit", 201),
        Query::new().with("limit", "01"),
        Query::new().with("include", "appInfos"),
        Query::new().with("fields", "unknown"),
        Query::new().with("limit", 1).with("limit", 2),
    ] {
        assert_eq!(client.apps(&q).await.unwrap_err().code, "invalid_query");
    }
    for id in ["../other", "x?secret", "x/y", "%2F", ""] {
        assert_eq!(
            client.app(id, &Query::new()).await.unwrap_err().code,
            "invalid_path"
        );
    }
    for attrs in [
        json!({}),
        json!([]),
        json!({"id":"rating1"}),
        json!({"gambling":null}),
        json!({"gambling":"false"}),
        json!({"kidsAgeBand":"INVALID"}),
        json!({"developerAgeRatingInfoUrl":"https://user:pass@example.com"}),
        json!({"ageRatingOverride":"NONE"}),
    ] {
        assert_eq!(
            client
                .update_age_rating("app1", "info1", &attrs)
                .await
                .unwrap_err()
                .code,
            "invalid_request"
        );
    }
    assert_eq!(
        client
            .get::<Value>("/sales-reports", &Query::new())
            .await
            .unwrap_err()
            .code,
        "route_not_found"
    );
    assert_eq!(
        Client::new("issuer", "key", "private-secret")
            .unwrap_err()
            .code,
        "invalid_credentials"
    );
}

#[test]
fn cursors_are_signed_expiring_and_client_bound() {
    let (pem, _) = pem();
    let client = Client::new("issuer", "key", &pem).unwrap();
    let other = Client::new("issuer", "key", &pem).unwrap();
    let mut cursor = pagination::Cursor {
        binding: "binding".into(),
        next: "apple-cursor".into(),
        seen: vec!["first-page".into()],
        expires: auth::now() + 900,
    };
    let token = client.encode_cursor(&cursor).unwrap();
    assert_eq!(
        client.decode_cursor(&token, "binding").unwrap().next,
        "apple-cursor"
    );
    assert_eq!(
        other.decode_cursor(&token, "binding").err().unwrap().code,
        "invalid_cursor"
    );
    assert!(client.decode_cursor(&token, "different").is_err());
    assert!(client.decode_cursor(&(token + "a"), "binding").is_err());
    cursor.expires = auth::now();
    assert!(client
        .decode_cursor(&client.encode_cursor(&cursor).unwrap(), "binding")
        .is_err());
}

#[tokio::test]
async fn pagination_cycles_and_resolver_pages_are_bounded() {
    let (client, task) = mock(vec![
        Step::get("/v1/apps", page(vec![], Some("?cursor=repeat"))).query(&[("limit", "50")]),
        Step::get("/v1/apps", page(vec![], Some("?cursor=repeat")))
            .query(&[("limit", "50"), ("cursor", "repeat")]),
    ])
    .await;
    let first = client.apps(&Query::new()).await.unwrap();
    assert_eq!(
        client
            .apps(&Query::new().with("cursor", first.next_cursor.unwrap()))
            .await
            .unwrap_err()
            .code,
        "upstream_incomplete_result"
    );
    task.await.unwrap();
    let mut steps = Vec::new();
    for n in 0..20 {
        let mut step = Step::get(
            "/v1/apps/app1/appInfos",
            page(vec![], Some(&format!("?cursor=page{}", n + 1))),
        )
        .query(&[("limit", "200")]);
        if n > 0 {
            step.query.insert("cursor".into(), format!("page{n}"));
        }
        steps.push(step);
    }
    let (client, task) = mock(steps).await;
    assert_eq!(
        client
            .app_info("app1", "info1", &Query::new())
            .await
            .unwrap_err()
            .code,
        "upstream_incomplete_result"
    );
    task.await.unwrap();
}
