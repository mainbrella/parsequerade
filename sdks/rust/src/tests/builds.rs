use super::*;

fn draft() -> Value {
    data(
        "appStoreVersions",
        "draft1",
        json!({"platform":"IOS", "versionString":"2.0", "appVersionState":"PREPARE_FOR_SUBMISSION"}),
    )
}

fn build() -> Value {
    data(
        "builds",
        "build1",
        json!({"version":"42", "processingState":"VALID", "expired":false, "buildAudienceType":"APP_STORE_ELIGIBLE"}),
    )
}

fn membership() -> Step {
    Step::get("/v1/apps/app1/appStoreVersions", page(vec![draft()], None))
        .query(&[("limit", "200"), ("filter[id]", "draft1")])
}

fn build_membership() -> Step {
    Step::get("/v1/builds", page(vec![build()], None)).query(&[
        ("limit", "200"),
        ("filter[app]", "app1"),
        ("filter[id]", "build1"),
    ])
}

fn preflight() -> Vec<Step> {
    vec![
        membership(),
        Step::get("/v1/appStoreVersions/draft1", json!({"data":draft()})),
        build_membership(),
        Step::get("/v1/builds/build1", json!({"data":build()})),
        build_membership(),
        Step::get(
            "/v1/builds/build1/preReleaseVersion",
            single(
                "preReleaseVersions",
                "pre1",
                json!({"version":"2.0", "platform":"IOS"}),
            ),
        ),
    ]
}

fn patch(payload: Value) -> Step {
    Step::get("/v1/appStoreVersions/draft1", payload).patch(json!({"data":{
        "type":"appStoreVersions", "id":"draft1",
        "relationships":{"build":{"data":{"type":"builds", "id":"build1"}}}
    }}))
}

#[tokio::test]
async fn build_filters_pagination_and_flat_types_follow_the_documented_contract() {
    let parameters = [
        ("filter[app]", "app1"),
        ("filter[preReleaseVersion.platform]", "IOS"),
        ("filter[preReleaseVersion.version]", "2.0"),
        ("sort", "-uploadedDate"),
        ("limit", "1"),
    ];
    let mut more = parameters.to_vec();
    more.push(("cursor", "next"));
    let (client, task) = mock(vec![
        Step::get("/v1/builds", page(vec![build()], Some("/v1/builds?filter%5Bapp%5D=app1&filter%5BpreReleaseVersion.platform%5D=IOS&filter%5BpreReleaseVersion.version%5D=2.0&sort=-uploadedDate&limit=1&cursor=next"))).query(&parameters),
        Step::get("/v1/builds", page(vec![], None)).query(&more),
    ]).await;
    let query = Query::new()
        .with("platform", "IOS")
        .with("prereleaseVersion", "2.0")
        .with("sort", "-uploadedDate")
        .with("limit", 1);
    let first = client.builds("app1", &query).await.unwrap();
    assert_eq!(first.items[0].app_id, "app1");
    assert_eq!(first.items[0].processing_state.as_deref(), Some("VALID"));
    let last = client
        .builds("app1", &query.with("cursor", first.next_cursor.unwrap()))
        .await
        .unwrap();
    assert!(last.items.is_empty());
    assert!(last.next_cursor.is_none());
    task.await.unwrap();
}

#[tokio::test]
async fn attaches_once_then_confirms_the_selected_build() {
    let mut steps = preflight();
    steps.extend([
        patch(json!({"data":draft()})),
        membership(),
        Step::get("/v1/appStoreVersions/draft1/build", json!({"data":build()})),
    ]);
    let (client, task) = mock(steps).await;
    let version = client
        .attach_build("app1", "draft1", "build1")
        .await
        .unwrap();
    assert_eq!(version["buildId"], "build1");
    assert_eq!(version["appId"], "app1");
    task.await.unwrap();
}

#[tokio::test]
async fn wrong_app_build_never_reaches_a_write() {
    let (client, task) = mock(vec![
        membership(),
        Step::get("/v1/appStoreVersions/draft1", json!({"data":draft()})),
        Step::get("/v1/builds", page(vec![], None)).query(&[
            ("limit", "200"),
            ("filter[app]", "app1"),
            ("filter[id]", "build1"),
        ]),
    ])
    .await;
    assert_eq!(
        client
            .attach_build("app1", "draft1", "build1")
            .await
            .unwrap_err()
            .status,
        404
    );
    task.await.unwrap();
}

#[tokio::test]
async fn non_draft_versions_and_unavailable_or_mismatched_builds_never_write() {
    for case in [
        "released",
        "processing",
        "expired",
        "internal",
        "version",
        "platform",
    ] {
        let mut version = draft();
        let mut candidate = build();
        if case == "released" {
            version["attributes"]["appVersionState"] = json!("READY_FOR_DISTRIBUTION");
        }
        if case == "processing" {
            candidate["attributes"]["processingState"] = json!("PROCESSING");
        }
        if case == "expired" {
            candidate["attributes"]["expired"] = json!(true);
        }
        if case == "internal" {
            candidate["attributes"]["buildAudienceType"] = json!("INTERNAL_ONLY");
        }
        let mut steps = vec![
            membership(),
            Step::get("/v1/appStoreVersions/draft1", json!({"data":version})),
        ];
        if case != "released" {
            steps.extend([
                build_membership(),
                Step::get("/v1/builds/build1", json!({"data":candidate})),
            ]);
        }
        if matches!(case, "version" | "platform") {
            steps.extend([build_membership(), Step::get("/v1/builds/build1/preReleaseVersion",single("preReleaseVersions","pre1",json!({"version":if case=="version" {"1.0"} else {"2.0"}, "platform":if case=="platform" {"MAC_OS"} else {"IOS"}})))]);
        }
        let (client, task) = mock(steps).await;
        assert_eq!(
            client
                .attach_build("app1", "draft1", "build1")
                .await
                .unwrap_err()
                .status,
            409,
            "{case}"
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn rejected_and_uncertain_writes_are_never_retried_or_reported_as_success() {
    for status in [409, 422, 503, 200] {
        let mut steps = preflight();
        steps.push(
            patch(if status == 200 {
                json!({"data":draft()})
            } else {
                json!({"errors":[]})
            })
            .status(status),
        );
        if status == 200 {
            steps.extend([
                membership(),
                Step::get(
                    "/v1/appStoreVersions/draft1/build",
                    single("builds", "other-build", json!({})),
                ),
            ]);
        }
        let (client, task) = mock(steps).await;
        let error = client
            .attach_build("app1", "draft1", "build1")
            .await
            .unwrap_err();
        assert!(!error.retryable);
        if status == 200 {
            assert_eq!(error.code, "write_outcome_unknown");
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn missing_attached_build_is_an_optional_not_found_and_queries_validate_locally() {
    let (client, task) = mock(vec![
        membership(),
        Step::get("/v1/appStoreVersions/draft1/build", json!({"data":null})),
    ])
    .await;
    assert_eq!(
        client
            .version_build("app1", "draft1", &Query::new())
            .await
            .unwrap_err()
            .status,
        404
    );
    assert_eq!(
        client
            .builds("app1", &Query::new().with("platform", "ANDROID"))
            .await
            .unwrap_err()
            .code,
        "invalid_query"
    );
    assert!(client
        .attach_build("../app", "draft1", "build1")
        .await
        .is_err());
    task.await.unwrap();
}
