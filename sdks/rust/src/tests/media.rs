use super::*;

fn linked(mut value: Value, relation: &str, kind: &str, id: &str) -> Value {
    value["relationships"] = json!({relation: {"data": {"type": kind, "id": id}}});
    value
}

fn localization(items: Vec<Value>, next: Option<&str>) -> Step {
    Step::get(
        "/v1/appStoreVersions/version1/appStoreVersionLocalizations",
        page(items, next),
    )
    .query(&[
        ("limit", "200"),
        ("filter[locale]", "en-US"),
        ("fields[appStoreVersionLocalizations]", "locale"),
    ])
}

fn selected_locale() -> Step {
    localization(
        vec![linked(
            data(
                "appStoreVersionLocalizations",
                "loc1",
                json!({"locale":"en-US"}),
            ),
            "appStoreVersion",
            "appStoreVersions",
            "version1",
        )],
        None,
    )
}

#[tokio::test]
async fn media_sets_resolve_scope_and_exact_locale_and_translate_queries() {
    let (client, task) = mock(vec![
        version_parent(),
        localization(
            vec![data(
                "appStoreVersionLocalizations",
                "other",
                json!({"locale":"en-GB"}),
            )],
            Some("?cursor=second"),
        ),
        selected_locale().query(&[
            ("limit", "200"),
            ("filter[locale]", "en-US"),
            ("fields[appStoreVersionLocalizations]", "locale"),
            ("cursor", "second"),
        ]),
        Step::get(
            "/v1/appStoreVersionLocalizations/loc1/appScreenshotSets",
            page(
                vec![linked(
                    data(
                        "appScreenshotSets",
                        "screens1",
                        json!({"screenshotDisplayType":"APP_IPHONE_67"}),
                    ),
                    "appStoreVersionLocalization",
                    "appStoreVersionLocalizations",
                    "loc1",
                )],
                None,
            ),
        )
        .query(&[
            ("limit", "200"),
            ("filter[screenshotDisplayType]", "APP_IPHONE_67"),
            ("fields[appScreenshotSets]", "screenshotDisplayType"),
        ]),
        version_parent(),
        selected_locale(),
        Step::get(
            "/v1/appStoreVersionLocalizations/loc1/appPreviewSets",
            page(
                vec![data(
                    "appPreviewSets",
                    "previews1",
                    json!({"previewType":"FUTURE_DEVICE"}),
                )],
                None,
            ),
        )
        .query(&[("limit", "50"), ("filter[previewType]", "IPHONE_67")]),
    ])
    .await;
    let sets = client
        .screenshot_sets(
            "app1",
            "version1",
            "en-US",
            &Query::new()
                .with("limit", 200)
                .with("screenshotDisplayType", "APP_IPHONE_67")
                .with("fields", "screenshotDisplayType"),
        )
        .await
        .unwrap();
    assert_eq!(sets.items[0].app_id, "app1");
    assert_eq!(sets.items[0].version_id, "version1");
    assert_eq!(
        sets.items[0].app_store_version_localization_id.as_deref(),
        Some("loc1")
    );
    assert_eq!(
        sets.items[0].screenshot_display_type.as_deref(),
        Some("APP_IPHONE_67")
    );
    let previews: Value = client
        .get(
            "/apps/app1/versions/version1/localizations/en-US/preview-sets",
            &Query::new().with("previewType", "IPHONE_67"),
        )
        .await
        .unwrap();
    assert_eq!(previews["items"][0]["previewType"], "FUTURE_DEVICE");
    task.await.unwrap();
}

#[tokio::test]
async fn media_pages_preserve_nested_images_states_nulls_and_order() {
    let image = json!({"templateUrl":"https://example.test/{w}x{h}{c}.{f}", "width":1290, "height":2796, "futureField":true});
    let (client, task) = mock(vec![
        Step::get(
            "/v1/appScreenshotSets/screens1/appScreenshots",
            page(vec![linked(
                data("appScreenshots", "shot2", json!({"fileName":"second.png", "fileSize":184320, "imageAsset":image, "assetDeliveryState":{"state":"COMPLETE","warnings":[],"errors":[]}, "sourceFileChecksum":null})),
                "appScreenshotSet", "appScreenshotSets", "screens1",
            )], Some("?cursor=more")),
        ).query(&[("limit", "1")]),
        Step::get(
            "/v1/appScreenshotSets/screens1/appScreenshots",
            page(vec![data("appScreenshots", "shot1", json!({"fileName":"first.png", "imageAsset":null}))], None),
        ).query(&[("limit", "1"), ("cursor", "more")]),
        Step::get(
            "/v1/appPreviewSets/previews1/appPreviews",
            page(vec![linked(
                data("appPreviews", "preview1", json!({"fileName":"demo.mov", "fileSize":12345, "videoUrl":"https://example.test/video.mp4", "mimeType":"video/quicktime", "previewFrameImage":{"image":image,"state":{"state":"COMPLETE"}}, "videoDeliveryState":{"state":"PROCESSING","warnings":[{"code":"WARNING","description":"Processing"}], "errors":[]}, "uploadOperations":[{"method":"PUT","url":"https://example.test/upload","length":12345,"offset":0,"requestHeaders":[{"name":"Content-Type","value":"video/quicktime"}]}]})),
                "appPreviewSet", "appPreviewSets", "previews1",
            )], None),
        ).query(&[("limit", "50")]),
    ]).await;
    let first: Page<Value> = client
        .get(
            "/screenshot-sets/screens1/screenshots",
            &Query::new().with("limit", 1),
        )
        .await
        .unwrap();
    assert_eq!(first.items[0]["id"], "shot2");
    assert_eq!(first.items[0]["imageAsset"], image);
    assert_eq!(first.items[0]["sourceFileChecksum"], Value::Null);
    assert!(first.items[0].get("appId").is_none());
    let second = client
        .screenshots(
            "screens1",
            &Query::new()
                .with("limit", 1)
                .with("cursor", first.next_cursor.unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(second.items[0].id, "shot1");
    assert_eq!(second.items[0].image_asset, None);
    assert_eq!(second.next_cursor, None);
    let previews = client.previews("previews1", &Query::new()).await.unwrap();
    let preview = &previews.items[0];
    assert_eq!(
        preview.video_url.as_deref(),
        Some("https://example.test/video.mp4")
    );
    assert_eq!(
        preview
            .preview_frame_image
            .as_ref()
            .unwrap()
            .image
            .as_ref()
            .unwrap()
            .width,
        Some(1290)
    );
    assert_eq!(
        preview
            .video_delivery_state
            .as_ref()
            .unwrap()
            .state
            .as_deref(),
        Some("PROCESSING")
    );
    assert_eq!(
        preview.upload_operations.as_ref().unwrap()[0].offset,
        Some(0)
    );
    assert_eq!(preview.app_preview_set_id.as_deref(), Some("previews1"));
    task.await.unwrap();
}

#[tokio::test]
async fn media_exact_locale_is_required_and_unique_across_pages() {
    for (items, next_items, expected) in [
        (
            vec![data(
                "appStoreVersionLocalizations",
                "other",
                json!({"locale":"en-GB"}),
            )],
            None,
            "localization_not_found",
        ),
        (
            vec![data(
                "appStoreVersionLocalizations",
                "loc1",
                json!({"locale":"en-US"}),
            )],
            Some(vec![data(
                "appStoreVersionLocalizations",
                "loc2",
                json!({"locale":"en-US"}),
            )]),
            "ambiguous_selection",
        ),
    ] {
        let next = next_items.as_ref().map(|_| "?cursor=more");
        let mut steps = vec![version_parent(), localization(items, next)];
        if let Some(items) = next_items {
            steps.push(localization(items, None).query(&[
                ("limit", "200"),
                ("filter[locale]", "en-US"),
                ("fields[appStoreVersionLocalizations]", "locale"),
                ("cursor", "more"),
            ]));
        }
        let (client, task) = mock(steps).await;
        assert_eq!(
            client
                .screenshot_sets("app1", "version1", "en-US", &Query::new())
                .await
                .unwrap_err()
                .code,
            expected
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn media_rejects_foreign_parents_before_returning_data() {
    let wrong_localization = linked(
        data(
            "appStoreVersionLocalizations",
            "loc1",
            json!({"locale":"en-US"}),
        ),
        "appStoreVersion",
        "appStoreVersions",
        "foreign",
    );
    let (client, task) = mock(vec![
        version_parent(),
        localization(vec![wrong_localization], None),
    ])
    .await;
    assert_eq!(
        client
            .preview_sets("app1", "version1", "en-US", &Query::new())
            .await
            .unwrap_err()
            .code,
        "upstream_invalid_response"
    );
    task.await.unwrap();

    let wrong_set = linked(
        data("appScreenshotSets", "screens1", json!({})),
        "appStoreVersionLocalization",
        "appStoreVersionLocalizations",
        "foreign",
    );
    let (client, task) = mock(vec![
        version_parent(),
        selected_locale(),
        Step::get(
            "/v1/appStoreVersionLocalizations/loc1/appScreenshotSets",
            page(vec![wrong_set], None),
        )
        .query(&[("limit", "50")]),
    ])
    .await;
    assert_eq!(
        client
            .screenshot_sets("app1", "version1", "en-US", &Query::new())
            .await
            .unwrap_err()
            .code,
        "upstream_invalid_response"
    );
    task.await.unwrap();

    for (path, apple_path, kind, relation, parent_kind) in [
        (
            "/screenshot-sets/set1/screenshots",
            "/v1/appScreenshotSets/set1/appScreenshots",
            "appScreenshots",
            "appScreenshotSet",
            "appScreenshotSets",
        ),
        (
            "/preview-sets/set1/previews",
            "/v1/appPreviewSets/set1/appPreviews",
            "appPreviews",
            "appPreviewSet",
            "appPreviewSets",
        ),
    ] {
        let wrong_child = linked(
            data(kind, "child1", json!({})),
            relation,
            parent_kind,
            "foreign",
        );
        let (client, task) = mock(vec![
            Step::get(apple_path, page(vec![wrong_child], None)).query(&[("limit", "50")])
        ])
        .await;
        assert_eq!(
            client
                .get::<Value>(path, &Query::new())
                .await
                .unwrap_err()
                .code,
            "upstream_invalid_response"
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn media_validates_nested_shapes_and_projects_only_requested_attributes() {
    for attributes in [
        json!({"imageAsset":{"width":"1290"}}),
        json!({"fileSize":1.5}),
        json!({"assetDeliveryState":{"errors":["bad"]}}),
        json!({"uploadOperations":{"method":"PUT"}}),
    ] {
        let (client, task) = mock(vec![Step::get(
            "/v1/appScreenshotSets/set1/appScreenshots",
            page(vec![data("appScreenshots", "shot1", attributes)], None),
        )
        .query(&[("limit", "50")])])
        .await;
        assert_eq!(
            client
                .screenshots("set1", &Query::new())
                .await
                .unwrap_err()
                .code,
            "upstream_invalid_response"
        );
        task.await.unwrap();
    }
    let (client, task) = mock(vec![Step::get(
        "/v1/appScreenshotSets/set1/appScreenshots",
        page(
            vec![data(
                "appScreenshots",
                "shot1",
                json!({"fileName":"screen.png","fileSize":12,"imageAsset":null}),
            )],
            None,
        ),
    )
    .query(&[
        ("limit", "50"),
        ("fields[appScreenshots]", "fileName,imageAsset"),
    ])])
    .await;
    let value: Value = client
        .get(
            "/screenshot-sets/set1/screenshots",
            &Query::new().with("fields", "fileName,imageAsset"),
        )
        .await
        .unwrap();
    assert_eq!(
        value,
        json!({"items":[{"id":"shot1","fileName":"screen.png","imageAsset":null}],"nextCursor":null})
    );
    task.await.unwrap();
}

#[tokio::test]
async fn media_rejects_unsupported_queries_and_paths_without_network() {
    let (client, task) = mock(vec![]).await;
    for query in [
        Query::new().with("limit", 201),
        Query::new().with("fields", "notAField"),
        Query::new().with("sort", "fileName"),
    ] {
        assert_eq!(
            client.screenshots("set1", &query).await.unwrap_err().code,
            "invalid_query"
        );
    }
    assert_eq!(
        client
            .screenshot_sets("app1", "version1", "en_US", &Query::new())
            .await
            .unwrap_err()
            .code,
        "invalid_path"
    );
    assert_eq!(
        client
            .previews("../foreign", &Query::new())
            .await
            .unwrap_err()
            .code,
        "invalid_path"
    );
    task.await.unwrap();
}
