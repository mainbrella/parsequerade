//! The same request/response scenarios run through the Python public API.
use super::*;

fn assert_subset(actual: &Value, expected: &Value, name: &str) {
    match expected {
        Value::Object(fields) => {
            assert!(actual.is_object(), "{name}: expected object, got {actual}");
            for (key, value) in fields {
                assert!(actual.get(key).is_some(), "{name}: missing {key}");
                assert_subset(&actual[key], value, name);
            }
        }
        Value::Array(items) => {
            let actual = actual.as_array().expect(name);
            assert_eq!(actual.len(), items.len(), "{name}");
            for (actual, expected) in actual.iter().zip(items) {
                assert_subset(actual, expected, name);
            }
        }
        _ => assert_eq!(actual, expected, "{name}"),
    }
}

#[tokio::test]
async fn shared_sdk_scenarios() {
    let scenarios: Vec<Value> = serde_json::from_str(include_str!("scenarios.json")).unwrap();
    for scenario in scenarios {
        let name = scenario["name"].as_str().unwrap();
        let steps = scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| {
                let mut result =
                    Step::get(step["path"].as_str().unwrap(), step["response"].clone());
                result.method = match step["method"].as_str().unwrap() {
                    "GET" => "GET",
                    "POST" => "POST",
                    "PATCH" => "PATCH",
                    method => panic!("Unexpected scenario method {method}"),
                };
                result.query = step["query"]
                    .as_object()
                    .map(|pairs| {
                        pairs
                            .iter()
                            .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(body) = step.get("body") {
                    result.body = Some(body.clone());
                }
                result.status = step["status"].as_u64().unwrap_or(200) as u16;
                result
            })
            .collect();
        let (client, task) = mock(steps).await;
        let args = scenario["args"].as_array().unwrap();
        let arg = |i: usize| args[i].as_str().unwrap();
        let result = match scenario["operation"].as_str().unwrap() {
            "get" => {
                let mut query = Query::new();
                if let Some(pairs) = args.get(1).and_then(Value::as_object) {
                    for (key, value) in pairs {
                        query = query.with(
                            key,
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string()),
                        );
                    }
                }
                client.get::<Value>(arg(0), &query).await
            }
            "update_metadata" => client.update_metadata(arg(0), &args[1]).await,
            "create_version" => client.create_version(arg(0), arg(1), arg(2)).await,
            "attach_build" => client.attach_build(arg(0), arg(1), arg(2)).await,
            operation => panic!("Unexpected scenario operation {operation}"),
        };
        if let Some(expected) = scenario.get("error") {
            let error = result.expect_err(name);
            assert_subset(&serde_json::to_value(error).unwrap(), expected, name);
        } else {
            assert_subset(&result.expect(name), &scenario["expect"], name);
        }
        task.await.unwrap();
    }
}
