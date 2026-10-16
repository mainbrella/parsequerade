use super::*;
use crate::tests::{mock, single, Step};

#[tokio::test]
async fn creates_one_draft_with_flat_response_and_never_retries_post() {
    for status in [201, 409, 503] {
        let body = json!({"data": {
            "type": "appStoreVersions", "attributes": {"platform": "IOS", "versionString": "1.2.8"},
            "relationships": {"app": {"data": {"type": "apps", "id": "app1"}}}
        }});
        let reply = if status == 201 {
            single("appStoreVersions", "draft1", json!({"platform":"IOS", "versionString":"1.2.8", "appVersionState":"PREPARE_FOR_SUBMISSION"}))
        } else { json!({"errors": []}) };
        let (client, task) = mock(vec![
            Step::get("/v1/apps/app1", single("apps", "app1", json!({}))),
            Step::get("/v1/appStoreVersions", reply).post(body).status(status),
        ]).await;
        let result = client.create_version("app1", "IOS", "1.2.8").await;
        if status == 201 {
            let flat = result.unwrap();
            assert_eq!(flat["id"], "draft1");
            assert_eq!(flat["appId"], "app1");
            assert_eq!(flat["versionString"], "1.2.8");
            assert!(flat.get("data").is_none());
        } else {
            assert!(!result.unwrap_err().retryable);
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn invalid_input_is_rejected_before_dispatch() {
    let (client, task) = mock(vec![]).await;
    for (app, platform, number) in [("../app1", "IOS", "1.0"), ("app1", "ANDROID", "1.0"), ("app1", "IOS", "1..2"), ("app1", "IOS", "1.0-beta")] {
        assert!(client.create_version(app, platform, number).await.is_err());
    }
    task.await.unwrap();
}
