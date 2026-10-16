use super::*;
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use std::{collections::BTreeMap, time::Instant};

fn client() -> u64 {
    let key = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
        .unwrap();
    let config = json!({"issuerId":"fixture", "keyId":"KEY123", "privateKey":format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",STANDARD.encode(key)),"customTransport":true});
    let result: Value = serde_json::from_str(&client_new_json(&config.to_string())).unwrap();
    result["client"].as_u64().unwrap()
}

fn start(client: u64, request: Value) -> u64 {
    let result: Value = serde_json::from_str(&start_json(client, &request.to_string())).unwrap();
    result["operation"].as_u64().unwrap()
}

fn event(operation: u64) -> Value {
    let start = Instant::now();
    loop {
        let value: Value = serde_json::from_str(&poll_json(operation, 50)).unwrap();
        if !value.is_null() {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "bridge event did not arrive"
        );
    }
}

fn subset(actual: &Value, expected: &Value) {
    match expected {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert!(actual.get(key).is_some(), "missing {key} in {actual}");
                subset(&actual[key], value);
            }
        }
        Value::Array(values) => {
            let actual = actual.as_array().unwrap();
            assert_eq!(actual.len(), values.len());
            for (a, b) in actual.iter().zip(values) {
                subset(a, b);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

#[test]
fn shared_scenarios_execute_through_host_bridge() {
    let scenarios: Vec<Value> = serde_json::from_str(include_str!("tests/scenarios.json")).unwrap();
    for scenario in scenarios {
        let client = client();
        let mut args = scenario["args"].clone();
        if scenario["operation"] == "get" {
            let query: Vec<_> = args[1]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| {
                    json!([
                        k,
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                    ])
                })
                .collect();
            args[1] = query.into();
        }
        let operation = start(
            client,
            json!({"operation":scenario["operation"],"args":args}),
        );
        // Existing operations retain their client even if its public handle is closed.
        client_free(client);
        for step in scenario["steps"].as_array().unwrap() {
            let request = event(operation);
            assert_eq!(request["kind"], "http", "{}", scenario["name"]);
            assert_eq!(request["method"], step["method"]);
            let url = parsequerade_url(request["url"].as_str().unwrap());
            assert_eq!(url.0, step["path"]);
            let query: BTreeMap<String, String> =
                serde_json::from_value(step.get("query").cloned().unwrap_or(json!({}))).unwrap();
            assert_eq!(url.1, query);
            assert!(request["headers"]["Authorization"]
                .as_str()
                .unwrap()
                .starts_with("Bearer "));
            let body = request["body"]
                .as_str()
                .map(|body| {
                    serde_json::from_slice::<Value>(&STANDARD.decode(body).unwrap()).unwrap()
                })
                .unwrap_or(Value::Null);
            assert_eq!(body, step.get("body").cloned().unwrap_or(Value::Null));
            let response = json!({"status":step["status"].as_u64().unwrap_or(200),"headers":{"Content-Type":"application/json"},"body":STANDARD.encode(step["response"].to_string())});
            assert!(respond_json(
                operation,
                request["id"].as_u64().unwrap(),
                &response.to_string()
            ));
        }
        let result = event(operation);
        assert_eq!(result["kind"], "result");
        if let Some(expected) = scenario.get("error") {
            subset(&result["error"], expected);
        } else {
            subset(&result["value"], &scenario["expect"]);
        }
        operation_free(operation);
    }
}

fn parsequerade_url(url: &str) -> (String, BTreeMap<String, String>) {
    // URLs are already core-validated; decode test queries without another URL dependency.
    let (path, query) = url
        .strip_prefix("https://api.appstoreconnect.apple.com")
        .unwrap()
        .split_once('?')
        .unwrap_or((
            url.strip_prefix("https://api.appstoreconnect.apple.com")
                .unwrap(),
            "",
        ));
    let decode = |value: &str| {
        let mut bytes = Vec::new();
        let mut input = value.bytes();
        while let Some(byte) = input.next() {
            if byte == b'%' {
                let hi = (input.next().unwrap() as char).to_digit(16).unwrap();
                let lo = (input.next().unwrap() as char).to_digit(16).unwrap();
                bytes.push((hi * 16 + lo) as u8);
            } else {
                bytes.push(if byte == b'+' { b' ' } else { byte });
            }
        }
        String::from_utf8(bytes).unwrap()
    };
    (
        path.into(),
        query
            .split('&')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let (k, v) = part.split_once('=').unwrap();
                (decode(k), decode(v))
            })
            .collect(),
    )
}

#[test]
fn cancellation_and_release_are_safe_with_a_pending_host_request() {
    let client = client();
    let operation = start(client, json!({"operation":"get","args":["/apps",[]]}));
    let request = event(operation);
    cancel(operation);
    assert!(!respond_json(
        operation,
        request["id"].as_u64().unwrap(),
        "{}"
    ));
    let result = event(operation);
    assert_eq!(result["error"]["code"], "operation_cancelled");
    operation_free(operation);
    operation_free(operation);
    client_free(client);
    let closed: Value = serde_json::from_str(&start_json(
        client,
        r#"{"operation":"get","args":["/apps",[]]}"#,
    ))
    .unwrap();
    assert_eq!(closed["error"]["code"], "client_closed");
}

#[test]
fn cancelled_writes_are_uncertain_and_never_replayed() {
    let client = client();
    let operation = start(
        client,
        json!({"operation":"create_version","args":["app1","IOS","1.0"]}),
    );
    let request = event(operation);
    cancel(operation);
    let result = event(operation);
    assert_eq!(result["error"]["code"], "write_outcome_unknown");
    assert_eq!(result["error"]["retryable"], false);
    assert!(!respond_json(
        operation,
        request["id"].as_u64().unwrap(),
        "{}"
    ));
    operation_free(operation);
    client_free(client);
}

#[test]
fn malformed_requests_and_c_strings_fail_without_dispatch() {
    assert_eq!(abi_version(), 1);
    let response: Value = serde_json::from_str(&client_new_json("{}")).unwrap();
    assert_eq!(response["error"]["code"], "invalid_credentials");
    let client = client();
    for request in [
        r#"{"operation":"get","args":[]}"#,
        r#"{"operation":"unknown","args":[]}"#,
        r#"{"operation":"get","args":["/apps",[]],"secret":"hidden"}"#,
    ] {
        let response: Value = serde_json::from_str(&start_json(client, request)).unwrap();
        assert_eq!(response["error"]["code"], "invalid_request");
    }
    unsafe {
        let pointer = pq_client_new(std::ptr::null());
        assert_eq!(
            serde_json::from_str::<Value>(CStr::from_ptr(pointer).to_str().unwrap()).unwrap()
                ["error"]["code"],
            "invalid_credentials"
        );
        pq_string_free(pointer);
        pq_string_free(std::ptr::null_mut());
    }
    client_free(client);
}

#[test]
fn shared_uploads_preserve_checkpoints_and_write_outcomes() {
    let fixtures: Value = serde_json::from_str(include_str!("tests/uploads.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let client = client();
        let operation = start(
            client,
            json!({
                "operation":"upload_listing_image",
                "args":[fixtures["request"],case.get("checkpoint").cloned().unwrap_or(json!({}))],
                "png":fixtures["png"],
            }),
        );
        let steps = case["steps"].as_array().unwrap();
        let mut index = 0;
        let mut snapshots = Vec::new();
        loop {
            let next = event(operation);
            match next["kind"].as_str().unwrap() {
                "checkpoint" => {
                    snapshots.push(next["checkpoint"].clone());
                    assert_eq!(next["checkpoint"]["binding"], fixtures["binding"]);
                    let response = if case["persistFailure"] == true {
                        json!({"error":"checkpoint_persistence_failed"})
                    } else {
                        json!({"ok":true})
                    };
                    assert!(respond_json(
                        operation,
                        next["id"].as_u64().unwrap(),
                        &response.to_string()
                    ));
                }
                "http" => {
                    let step = &steps[index];
                    index += 1;
                    assert_eq!(next["method"], step["method"], "{}", case["name"]);
                    let (path, query) = parsequerade_url_flexible(next["url"].as_str().unwrap());
                    assert_eq!(path, step["path"]);
                    assert_eq!(serde_json::to_value(query).unwrap(), step["query"]);
                    let body = next["body"]
                        .as_str()
                        .map(|body| STANDARD.decode(body).unwrap());
                    if let Some(expected) = step["bodyBase64"].as_str() {
                        assert_eq!(body.unwrap(), STANDARD.decode(expected).unwrap());
                        assert!(next["headers"]
                            .as_object()
                            .unwrap()
                            .keys()
                            .all(|key| !key.eq_ignore_ascii_case("authorization")));
                    } else {
                        let value = body
                            .map(|body| serde_json::from_slice::<Value>(&body).unwrap())
                            .unwrap_or(Value::Null);
                        assert_eq!(value, step["body"]);
                    }
                    let response = if let Some(error) = step["error"].as_str() {
                        json!({"error":error})
                    } else {
                        json!({"status":step["status"],"headers":{"content-type":"application/json"},"body":STANDARD.encode(step["payload"].to_string())})
                    };
                    assert!(respond_json(
                        operation,
                        next["id"].as_u64().unwrap(),
                        &response.to_string()
                    ));
                }
                "result" => {
                    assert_eq!(index, steps.len(), "{}", case["name"]);
                    if case["persistFailure"] == true {
                        assert_eq!(next["error"]["code"], "checkpoint_persistence_failed");
                    } else if let Some(code) = case["error"].as_str() {
                        assert_eq!(next["error"]["code"], code);
                        assert_eq!(next["error"]["retryable"], false);
                    } else {
                        assert_eq!(next["value"], case["expect"]);
                    }
                    if let Some(last) = snapshots.last() {
                        assert_eq!(&next["checkpoint"], last);
                    }
                    break;
                }
                kind => panic!("Unexpected bridge event {kind}"),
            }
        }
        operation_free(operation);
        client_free(client);
    }
}

fn parsequerade_url_flexible(url: &str) -> (String, BTreeMap<String, String>) {
    if url.starts_with("https://api.appstoreconnect.apple.com") {
        parsequerade_url(url)
    } else {
        let rest = url.strip_prefix("https://").unwrap();
        let path = rest.find('/').map(|index| &rest[index..]).unwrap_or("/");
        parsequerade_url(&format!("https://api.appstoreconnect.apple.com{path}"))
    }
}

#[test]
fn expired_cancelled_and_abandoned_events_never_reach_host_dispatch() {
    for state in ["expired", "abandoned", "cancelled"] {
        let (events, receiver) = mpsc::channel();
        let operation = Arc::new(Operation {
            events,
            receiver: Mutex::new(receiver),
            responses: Mutex::new(HashMap::new()),
            checkpoint: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
            write: true,
        });
        let id = next_id();
        operations().lock().unwrap().insert(id, operation.clone());
        let response = operation
            .request(json!({"kind":"http","method":"PATCH","timeoutMs":1000}))
            .unwrap();
        match state {
            "expired" => {
                operation
                    .responses
                    .lock()
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap()
                    .deadline = Some(Instant::now());
                // Keep the receiver alive: deadline expiry itself must suppress dispatch.
                operation
                    .events
                    .send(json!({"kind":"result","error":{"code":"write_outcome_unknown"}}))
                    .unwrap();
                assert_eq!(event(id)["kind"], "result");
                assert_eq!(response.blocking_recv().unwrap()["error"], "timeout");
            }
            _ => {
                if state == "cancelled" {
                    operation.cancel();
                }
                drop(response);
                operation
                    .events
                    .send(json!({"kind":"result","error":{"code":"write_outcome_unknown"}}))
                    .unwrap();
                assert_eq!(event(id)["kind"], "result");
            }
        }
        operation_free(id);
    }
}
