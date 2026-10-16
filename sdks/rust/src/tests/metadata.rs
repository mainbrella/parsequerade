use super::*;

#[tokio::test]
async fn app_metadata_patch_sends_only_requested_attributes_and_merges_confirmation() {
    let attributes = json!({"contentRightsDeclaration":"DOES_NOT_USE_THIRD_PARTY_CONTENT"});
    let (client, task) = mock(vec![
        Step::get(
            "/v1/apps/app1",
            single(
                "apps",
                "app1",
                json!({"name":"Notes", "contentRightsDeclaration":"USES_THIRD_PARTY_CONTENT"}),
            ),
        ),
        Step::get(
            "/v1/apps/app1",
            single(
                "apps",
                "app1",
                json!({"contentRightsDeclaration":attributes["contentRightsDeclaration"]}),
            ),
        )
        .patch(json!({"data":{"type":"apps", "id":"app1", "attributes":attributes}})),
    ])
    .await;

    let result = client
        .update_metadata("/apps/app1", &attributes)
        .await
        .unwrap();
    assert_eq!(result["id"], "app1");
    assert_eq!(result["name"], "Notes");
    assert_eq!(
        result["contentRightsDeclaration"],
        attributes["contentRightsDeclaration"]
    );
    task.await.unwrap();
}

#[tokio::test]
async fn version_metadata_patch_updates_only_editable_version_fields() {
    let attributes = json!({"copyright":"© 2026", "versionString":"2.1"});
    let (client, task) = mock(vec![
        Step::get(
            "/v1/apps/app1/appStoreVersions",
            page(vec![data("appStoreVersions", "version1", json!({}))], None),
        )
        .query(&[("limit", "200"), ("filter[id]", "version1")]),
        Step::get(
            "/v1/appStoreVersions/version1",
            single(
                "appStoreVersions",
                "version1",
                json!({"versionString":"2.0", "copyright":"© 2025"}),
            ),
        ),
        Step::get(
            "/v1/appStoreVersions/version1",
            single("appStoreVersions", "version1", attributes.clone()),
        )
        .patch(
            json!({"data":{"type":"appStoreVersions", "id":"version1", "attributes":attributes}}),
        ),
    ])
    .await;

    let result = client
        .update_metadata("/apps/app1/versions/version1", &attributes)
        .await
        .unwrap();
    assert_eq!(result["appId"], "app1");
    assert_eq!(result["id"], "version1");
    assert_eq!(result["versionString"], "2.1");
    assert_eq!(result["copyright"], "© 2026");
    task.await.unwrap();
}

#[tokio::test]
async fn localization_patch_resolves_parent_and_exact_locale_and_allows_clearing() {
    let attributes = json!({"privacyPolicyUrl":""});
    let (client, task) = mock(vec![
        Step::get(
            "/v1/apps/app1/appInfos",
            page(vec![data("appInfos", "info1", json!({}))], None),
        )
        .query(&[("limit", "200")]),
        Step::get(
            "/v1/appInfos/info1/appInfoLocalizations",
            page(
                vec![data(
                    "appInfoLocalizations",
                    "loc1",
                    json!({"locale":"en-US"}),
                )],
                None,
            ),
        )
        .query(&[
            ("limit", "200"),
            ("filter[locale]", "en-US"),
            ("fields[appInfoLocalizations]", "locale"),
        ]),
        Step::get(
            "/v1/appInfoLocalizations/loc1",
            single(
                "appInfoLocalizations",
                "loc1",
                json!({"locale":"en-US", "privacyPolicyUrl":"https://example.test/privacy"}),
            ),
        ),
        Step::get(
            "/v1/appInfoLocalizations/loc1",
            single(
                "appInfoLocalizations",
                "loc1",
                json!({"locale":"en-US", "privacyPolicyUrl":""}),
            ),
        )
        .patch(
            json!({"data":{"type":"appInfoLocalizations", "id":"loc1", "attributes":attributes}}),
        ),
    ])
    .await;

    let result = client
        .update_metadata(
            "/apps/app1/app-infos/info1/localizations/en-US",
            &attributes,
        )
        .await
        .unwrap();
    assert_eq!(result["appId"], "app1");
    assert_eq!(result["appInfoId"], "info1");
    assert_eq!(result["id"], "loc1");
    assert_eq!(result["locale"], "en-US");
    assert_eq!(result["privacyPolicyUrl"], "");
    task.await.unwrap();
}

#[tokio::test]
async fn version_localization_patch_validates_locale_and_preserves_unreturned_attributes() {
    let attributes = json!({"description":"Updated description"});
    let (client, task) = mock(vec![
        Step::get(
            "/v1/apps/app1/appStoreVersions",
            page(vec![data("appStoreVersions", "version1", json!({}))], None),
        )
        .query(&[("limit", "200"), ("filter[id]", "version1")]),
        Step::get(
            "/v1/appStoreVersions/version1/appStoreVersionLocalizations",
            page(vec![data("appStoreVersionLocalizations", "vloc1", json!({"locale":"fr-FR"}))], None),
        )
        .query(&[
            ("limit", "200"),
            ("filter[locale]", "fr-FR"),
            ("fields[appStoreVersionLocalizations]", "locale"),
        ]),
        Step::get(
            "/v1/appStoreVersionLocalizations/vloc1",
            single("appStoreVersionLocalizations", "vloc1", json!({"locale":"fr-FR", "description":"Old", "keywords":"keep"})),
        ),
        Step::get(
            "/v1/appStoreVersionLocalizations/vloc1",
            single("appStoreVersionLocalizations", "vloc1", json!({"locale":"fr-FR", "description":"Updated description"})),
        )
        .patch(json!({"data":{"type":"appStoreVersionLocalizations", "id":"vloc1", "attributes":attributes}})),
    ])
    .await;

    let result = client
        .update_metadata(
            "/apps/app1/versions/version1/localizations/fr-FR",
            &attributes,
        )
        .await
        .unwrap();
    assert_eq!(result["appId"], "app1");
    assert_eq!(result["versionId"], "version1");
    assert_eq!(result["locale"], "fr-FR");
    assert_eq!(result["description"], "Updated description");
    assert_eq!(result["keywords"], "keep");
    task.await.unwrap();
}

#[tokio::test]
async fn unsupported_or_malformed_metadata_fails_without_network_requests() {
    let (client, task) = mock(vec![]).await;
    for (path, attributes) in [
        ("/apps/app1", json!({})),
        ("/apps/app1", json!({"name":"No"})),
        ("/apps/app1", json!({"contentRightsDeclaration":false})),
        ("/apps/app1/versions/version1", json!({"platform":"IOS"})),
        ("/apps/app1", json!(["contentRightsDeclaration"])),
        (
            "/apps/app1/app-infos/info1/age-rating",
            json!({"gambling":false}),
        ),
    ] {
        let error = client.update_metadata(path, &attributes).await.unwrap_err();
        assert_eq!(error.code, "invalid_update");
    }
    task.await.unwrap();
}

#[tokio::test]
async fn rejected_metadata_patch_is_not_retried() {
    let attributes = json!({"versionString":"2.0"});
    let (client, task) = mock(vec![
        Step::get(
            "/v1/apps/app1/appStoreVersions",
            page(vec![data("appStoreVersions", "version1", json!({}))], None),
        )
        .query(&[("limit", "200"), ("filter[id]", "version1")]),
        Step::get(
            "/v1/appStoreVersions/version1",
            single(
                "appStoreVersions",
                "version1",
                json!({"versionString":"1.0", "copyright":"© 2025"}),
            ),
        ),
        Step::get(
            "/v1/appStoreVersions/version1",
            json!({"errors":[{"code":"PRIVATE", "detail":"secret"}]}),
        )
        .status(503)
        .patch(
            json!({"data":{"type":"appStoreVersions", "id":"version1", "attributes":attributes}}),
        ),
    ])
    .await;

    let error = client
        .update_metadata("/apps/app1/versions/version1", &attributes)
        .await
        .unwrap_err();
    assert!(!error.retryable);
    assert!(!format!("{error:?}").contains("secret"));
    task.await.unwrap();
}

#[tokio::test]
async fn malformed_or_wrong_resource_patch_response_has_unknown_outcome() {
    for response in [
        single(
            "apps",
            "different",
            json!({"contentRightsDeclaration":"value"}),
        ),
        single(
            "appStoreVersions",
            "app1",
            json!({"contentRightsDeclaration":"value"}),
        ),
    ] {
        let attributes = json!({"contentRightsDeclaration":"value"});
        let (client, task) = mock(vec![
            Step::get("/v1/apps/app1", single("apps", "app1", json!({}))),
            Step::get("/v1/apps/app1", response)
                .patch(json!({"data":{"type":"apps", "id":"app1", "attributes":attributes}})),
        ])
        .await;

        let error = client
            .update_metadata("/apps/app1", &attributes)
            .await
            .unwrap_err();
        assert_eq!(error.code, "write_outcome_unknown");
        assert!(!error.retryable);
        task.await.unwrap();
    }
}
