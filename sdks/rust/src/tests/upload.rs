use super::*;

const HEADER: &[u8] = include_bytes!("fixtures/header.png");
const SEARCH: &[u8] = include_bytes!("fixtures/search.png");

fn request(placement_type: ListingImagePlacementType) -> ListingImageUpload {
    ListingImageUpload {
        app_id: "app1".into(),
        version_id: "version1".into(),
        locale: "en-US".into(),
        placement_type,
        file_name: "listing.png".into(),
    }
}

fn linked(mut value: Value, relation: &str, kind: &str, id: &str) -> Value {
    value["relationships"] = json!({relation:{"data":{"type":kind,"id":id}}});
    value
}

fn selected_locale() -> Step {
    Step::get(
        "/v1/appStoreVersions/version1/appStoreVersionLocalizations",
        page(
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
        ),
    )
    .query(&[
        ("limit", "200"),
        ("filter[locale]", "en-US"),
        ("fields[appStoreVersionLocalizations]", "locale"),
    ])
}

fn scope() -> Vec<Step> {
    vec![
        version_parent(),
        selected_locale(),
        Step::get(
            "/v1/appStoreVersionLocalizations/loc1",
            single(
                "appStoreVersionLocalizations",
                "loc1",
                json!({"locale":"en-US"}),
            ),
        )
        .query(&[("fields[appStoreVersionLocalizations]", "locale")]),
    ]
}

fn placements(items: Vec<Value>) -> Vec<Step> {
    vec![
        version_parent(),
        selected_locale(),
        Step::get(
            "/v1/appStoreVersionLocalizations/loc1/placements",
            page(items, None),
        )
        .query(&[
            ("limit", "200"),
            ("filter[placementType]", "PRODUCT_PAGE_HEADER_ASSET"),
            ("include", "image,video"),
        ]),
    ]
}

fn placement(image_id: &str) -> Value {
    let mut value = linked(
        data(
            "appAssetLibraryPlacements",
            "placement1",
            json!({
                "mediaType":"IMAGE", "placementType":"PRODUCT_PAGE_HEADER_ASSET", "placementGroup":"CREATIVE_IOS", "state":"ASSET_PROCESSING"
            }),
        ),
        "appStoreVersionLocalization",
        "appStoreVersionLocalizations",
        "loc1",
    );
    value["relationships"]["image"] =
        json!({"data":{"type":"appAssetLibraryImages","id":image_id}});
    value
}

fn catalog() -> Value {
    page(
        vec![data(
            "appAssetLibraryRefData",
            "reference1",
            json!({
                "features":[{"featureId":"APP_STORE_VERSIONS","placementPolicies":[{"placementType":"PRODUCT_PAGE_HEADER_ASSET","groupLimits":[{"groupIds":["CREATIVE_IOS"],"maxCount":1}]}]}],
                "placementProfileGroups":[{"placementProfileGroupId":"CREATIVE_IOS","platform":"IOS"}],
                "placementTypes":[{"placementTypeId":"PRODUCT_PAGE_HEADER_ASSET","acceptsAssetCategories":["CREATIVE_ASSETS"],"specMappings":[{"placementGroupId":"CREATIVE_IOS","specs":["header-spec"]}]}],
                "imageSpecs":[{"specId":"header-spec","dimensions":{"minWidth":3840,"maxWidth":3840,"minHeight":1646,"maxHeight":1646},"compatiblePlacementTypes":["PRODUCT_PAGE_HEADER_ASSET"],"alphaAllowed":false,"fileExtensions":[".png"],"mimeTypes":["image/png"],"maxFileSize":33554432}]
            }),
        )],
        None,
    )
}

fn library_and_catalog() -> Vec<Step> {
    vec![
        Step::get(
            "/v1/apps/app1/assetLibrary",
            single("appAssetLibraries", "library-different-from-app", json!({})),
        ),
        Step::get("/v1/appAssetLibraryRefData", catalog()),
    ]
}

#[tokio::test]
async fn upload_rejects_foreign_version_before_any_mutation() {
    let foreign = linked(
        data("appStoreVersions", "version1", json!({})),
        "app",
        "apps",
        "another-app",
    );
    let (client, task) = mock(vec![Step::get(
        "/v1/apps/app1/appStoreVersions",
        page(vec![foreign], None),
    )
    .query(&[("limit", "200"), ("filter[id]", "version1")])])
    .await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut ListingImageUploadCheckpoint::default(),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "upstream_invalid_response");
    task.await.unwrap();
}

#[tokio::test]
async fn upload_existing_placement_stops_before_reservation() {
    let mut steps = scope();
    steps.extend(placements(vec![placement("existing-image")]));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut ListingImageUploadCheckpoint::default(),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "existing_placement");
    assert!(!error.retryable);
    task.await.unwrap();
}

#[tokio::test]
async fn upload_invalid_pngs_and_dimensions_make_no_requests() {
    let (client, task) = mock(vec![]).await;
    for bytes in [&HEADER[..24], SEARCH, b"not PNG"] {
        assert!(client
            .upload_listing_image(
                &request(ListingImagePlacementType::ProductPageHeader),
                bytes,
                &mut ListingImageUploadCheckpoint::default(),
                |_| Ok(())
            )
            .await
            .is_err());
    }
    task.await.unwrap();
}

fn reference_name() -> String {
    use ring::digest::{digest, SHA256};
    let sha = |bytes: &[u8]| {
        digest(&SHA256, bytes)
            .as_ref()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>()
    };
    let binding = format!(
        "app1:version1:en-US:PRODUCT_PAGE_HEADER_ASSET:listing.png:{}",
        sha(HEADER)
    );
    format!("parsequerade-{}", sha(binding.as_bytes()))
}

fn reserve_body() -> Value {
    json!({"data":{"type":"appAssetLibraryImages","attributes":{"fileName":"listing.png","fileSize":HEADER.len(),"category":"CREATIVE_ASSETS","referenceName":reference_name()},"relationships":{"assetLibrary":{"data":{"type":"appAssetLibraries","id":"library-different-from-app"}}}}})
}

fn upload_operation(path: &str, offset: usize, length: usize) -> Value {
    json!({"method":"PUT","url":format!("__MOCK_ORIGIN__{path}"),"offset":offset,"length":length,
        "requestHeaders":[{"name":"Content-Type","value":"image/png"},{"name":"X-Upload-Token","value":"signed-fixture"}]})
}

fn reservation(operations: Vec<Value>) -> Step {
    Step::get(
        "/v1/appAssetLibraryImages",
        single(
            "appAssetLibraryImages",
            "image1",
            json!({"state":"AWAITING_UPLOAD","uploadOperations":operations}),
        ),
    )
    .post(reserve_body())
}

fn preflight() -> Vec<Step> {
    let mut steps = scope();
    steps.extend(placements(vec![]));
    steps.extend(library_and_catalog());
    steps
}

fn commit() -> Step {
    Step::get("/v1/appAssetLibraryImages/image1",single("appAssetLibraryImages","image1",json!({"state":"UPLOAD_COMPLETE"})))
        .patch(json!({"data":{"type":"appAssetLibraryImages","id":"image1","attributes":{"uploaded":true}}}))
}

fn create_placement() -> Step {
    Step::get("/v1/appAssetLibraryPlacements",single("appAssetLibraryPlacements","placement1",json!({"state":"ASSET_PROCESSING"})))
        .post(json!({"data":{"type":"appAssetLibraryPlacements","attributes":{"placementType":"PRODUCT_PAGE_HEADER_ASSET","placementGroup":"CREATIVE_IOS"},"relationships":{"image":{"data":{"type":"appAssetLibraryImages","id":"image1"}},"appStoreVersionLocalization":{"data":{"type":"appStoreVersionLocalizations","id":"loc1"}}}}}))
}

#[tokio::test]
async fn upload_sends_exact_chunks_and_commit_and_verifies_placement() {
    let half = HEADER.len() / 2;
    let mut steps = preflight();
    steps.push(reservation(vec![
        upload_operation("/upload/second", half, HEADER.len() - half),
        upload_operation("/upload/first", 0, half),
    ]));
    steps.push(
        Step::get("/upload/second", json!({}))
            .put(&HEADER[half..])
            .request_header("content-type", "image/png")
            .request_header("x-upload-token", "signed-fixture"),
    );
    steps.push(
        Step::get("/upload/first", json!({}))
            .put(&HEADER[..half])
            .request_header("content-type", "image/png")
            .request_header("x-upload-token", "signed-fixture"),
    );
    steps.push(commit());
    steps.extend(placements(vec![]));
    steps.push(create_placement());
    steps.extend(placements(vec![placement("image1")]));
    let (client, task) = mock(steps).await;
    let mut checkpoint = ListingImageUploadCheckpoint::default();
    let mut saves = Vec::new();
    let result = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut checkpoint,
            |c| {
                saves.push(serde_json::to_value(c).unwrap());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"appId":"app1","versionId":"version1","locale":"en-US","placementType":"PRODUCT_PAGE_HEADER_ASSET","imageId":"image1","placementId":"placement1"})
    );
    assert!(saves
        .iter()
        .any(|v| v["phase"] == "reservation_started" && v.get("imageId").is_none()));
    assert!(saves
        .iter()
        .any(|v| v["phase"] == "commit_started" && v["imageId"] == "image1"));
    assert!(saves
        .iter()
        .any(|v| v["phase"] == "placement_started" && v["imageId"] == "image1"));
    let serialized = serde_json::to_string(&checkpoint).unwrap();
    assert!(!serialized.contains("signed-fixture"));
    assert!(!serialized.contains("__MOCK_ORIGIN__"));
    task.await.unwrap();
}

#[tokio::test]
async fn malformed_upload_operations_never_dispatch_put_or_commit() {
    let mut malicious = upload_operation("/upload", 0, HEADER.len());
    malicious["requestHeaders"] = json!([{"name":"Authorization","value":"Bearer malicious"}]);
    let mut wrong_host = upload_operation("/upload", 0, HEADER.len());
    wrong_host["url"] = "https://blobstore.apple.com.evil.invalid/upload".into();
    let mut redirect_host = upload_operation("/upload", 0, HEADER.len());
    redirect_host["url"] = "http://store-030.blobstore.apple.com/upload".into();
    for ops in [
        vec![upload_operation("/upload", 1, HEADER.len() - 1)],
        vec![
            upload_operation("/a", 0, HEADER.len()),
            upload_operation("/b", 0, 1),
        ],
        vec![upload_operation("/upload", 0, HEADER.len() + 1)],
        vec![malicious],
        vec![wrong_host],
        vec![redirect_host],
    ] {
        let mut steps = preflight();
        steps.push(reservation(ops));
        let (client, task) = mock(steps).await;
        let mut checkpoint = ListingImageUploadCheckpoint::default();
        let error = client
            .upload_listing_image(
                &request(ListingImagePlacementType::ProductPageHeader),
                HEADER,
                &mut checkpoint,
                |_| Ok(()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "upstream_invalid_response");
        assert_eq!(
            serde_json::to_value(checkpoint).unwrap()["imageId"],
            "image1"
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn uncertain_reservation_is_not_retried_and_checkpoint_blocks_duplicate() {
    let mut steps = preflight();
    let mut reserve = reservation(vec![upload_operation("/upload", 0, HEADER.len())]);
    reserve.disconnect = true;
    steps.push(reserve);
    let (client, task) = mock(steps).await;
    let mut checkpoint = ListingImageUploadCheckpoint::default();
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut checkpoint,
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    assert!(!error.retryable);
    task.await.unwrap();
    let (client, task) = mock(vec![]).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut checkpoint,
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    task.await.unwrap();
}

fn state_checkpoint(phase: &str) -> ListingImageUploadCheckpoint {
    use ring::digest::{digest, SHA256};
    let png_sha = digest(&SHA256, HEADER)
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let mut value = json!({"binding":format!("app1:version1:en-US:PRODUCT_PAGE_HEADER_ASSET:listing.png:{png_sha}"),"imageId":"image1","phase":phase});
    if phase.starts_with("placement_") {
        value["placementGroup"] = "CREATIVE_IOS".into();
    }
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn uncertain_placement_is_reconciled_from_readback_and_never_posted_again() {
    let mut steps = scope();
    steps.extend(placements(vec![placement("image1")]));
    steps.push(Step::get(
        "/v1/appAssetLibraryImages/image1",
        single(
            "appAssetLibraryImages",
            "image1",
            json!({"state":"UPLOAD_COMPLETE"}),
        ),
    ));
    let (client, task) = mock(steps).await;
    let mut checkpoint = state_checkpoint("placement_started");
    let result = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut checkpoint,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(result.placement_id, "placement1");
    assert_eq!(
        serde_json::to_value(&checkpoint).unwrap()["placementId"],
        "placement1"
    );
    task.await.unwrap();

    let mut steps = scope();
    steps.extend(placements(vec![]));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut state_checkpoint("placement_started"),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    assert!(!error.retryable);
    task.await.unwrap();
}

#[tokio::test]
async fn uncertain_commit_only_reads_and_never_repeats_patch() {
    let mut steps = preflight();
    steps.push(Step::get(
        "/v1/appAssetLibraryImages/image1",
        single(
            "appAssetLibraryImages",
            "image1",
            json!({"state":"AWAITING_UPLOAD"}),
        ),
    ));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut state_checkpoint("commit_started"),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    task.await.unwrap();
}

#[tokio::test]
async fn failed_image_cannot_produce_a_verified_receipt() {
    let mut steps = scope();
    steps.extend(placements(vec![placement("image1")]));
    steps.push(Step::get(
        "/v1/appAssetLibraryImages/image1",
        single("appAssetLibraryImages", "image1", json!({"state":"FAILED"})),
    ));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut state_checkpoint("placement_started"),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    task.await.unwrap();
}

#[tokio::test]
async fn placement_readback_must_match_the_created_image() {
    let mut steps = preflight();
    steps.push(reservation(vec![upload_operation(
        "/upload",
        0,
        HEADER.len(),
    )]));
    steps.push(Step::get("/upload", json!({})).put(HEADER));
    steps.push(commit());
    steps.extend(placements(vec![]));
    steps.push(create_placement());
    steps.extend(placements(vec![placement("another-image")]));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut ListingImageUploadCheckpoint::default(),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    assert!(!error.retryable);
    task.await.unwrap();
}

#[tokio::test]
async fn checkpoint_save_failure_prevents_reservation() {
    let (client, task) = mock(preflight()).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut ListingImageUploadCheckpoint::default(),
            |_| {
                Err(Error::new(
                    500,
                    "fixture_storage_failure",
                    "Could not save checkpoint.",
                ))
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "fixture_storage_failure");
    task.await.unwrap();
}

#[tokio::test]
async fn changed_png_cannot_reuse_a_checkpoint() {
    let (client, task) = mock(vec![]).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::SearchResults),
            SEARCH,
            &mut state_checkpoint("committed"),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "checkpoint_mismatch");
    task.await.unwrap();
}

#[tokio::test]
async fn committed_checkpoint_with_stale_image_read_never_repeats_commit() {
    let mut steps = preflight();
    steps.push(Step::get(
        "/v1/appAssetLibraryImages/image1",
        single(
            "appAssetLibraryImages",
            "image1",
            json!({"state":"AWAITING_UPLOAD"}),
        ),
    ));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut state_checkpoint("committed"),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    task.await.unwrap();
}

#[tokio::test]
async fn put_redirect_or_failure_does_not_commit_or_retry() {
    for status in [307, 500] {
        let mut steps = preflight();
        steps.push(reservation(vec![upload_operation(
            "/upload",
            0,
            HEADER.len(),
        )]));
        steps.push(
            Step::get("/upload", json!({}))
                .put(HEADER)
                .status(status)
                .header("Location", "https://evil.invalid/secret"),
        );
        let (client, task) = mock(steps).await;
        let error = client
            .upload_listing_image(
                &request(ListingImagePlacementType::ProductPageHeader),
                HEADER,
                &mut ListingImageUploadCheckpoint::default(),
                |_| Ok(()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "write_outcome_unknown");
        assert!(!error.retryable);
        task.await.unwrap();
    }
}

#[tokio::test]
async fn placement_that_appears_during_upload_prevents_create() {
    let mut steps = preflight();
    steps.push(reservation(vec![upload_operation(
        "/upload",
        0,
        HEADER.len(),
    )]));
    steps.push(Step::get("/upload", json!({})).put(HEADER));
    steps.push(commit());
    steps.extend(placements(vec![placement("another-image")]));
    let (client, task) = mock(steps).await;
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut ListingImageUploadCheckpoint::default(),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "existing_placement");
    task.await.unwrap();
}

#[tokio::test]
async fn uncertain_commit_transport_failure_does_not_retry() {
    let mut steps = preflight();
    steps.push(reservation(vec![upload_operation(
        "/upload",
        0,
        HEADER.len(),
    )]));
    steps.push(Step::get("/upload", json!({})).put(HEADER));
    let mut failed_commit = commit();
    failed_commit.disconnect = true;
    steps.push(failed_commit);
    let (client, task) = mock(steps).await;
    let mut checkpoint = ListingImageUploadCheckpoint::default();
    let error = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut checkpoint,
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "write_outcome_unknown");
    assert!(!error.retryable);
    assert_eq!(
        serde_json::to_value(checkpoint).unwrap()["phase"],
        "commit_started"
    );
    task.await.unwrap();
}

#[tokio::test]
async fn search_png_upload_uses_search_type_and_dimensions() {
    fn replace_type(value: &mut Value) {
        match value {
            Value::String(text) if text == "PRODUCT_PAGE_HEADER_ASSET" => {
                *text = "APP_STORE_SEARCH_RESULTS_ASSET".into()
            }
            Value::Array(values) => values.iter_mut().for_each(replace_type),
            Value::Object(map) => map.values_mut().for_each(replace_type),
            _ => {}
        }
    }
    let mut steps = preflight();
    let cat = steps.last_mut().unwrap();
    cat.payload["data"][0]["attributes"]["imageSpecs"][0]["dimensions"] =
        json!({"minWidth":1920,"maxWidth":1920,"minHeight":1280,"maxHeight":1280});
    let mut reserve = reservation(vec![upload_operation("/upload", 0, SEARCH.len())]);
    let png_sha = ring::digest::digest(&ring::digest::SHA256, SEARCH)
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let binding =
        format!("app1:version1:en-US:APP_STORE_SEARCH_RESULTS_ASSET:listing.png:{png_sha}");
    let binding_sha = ring::digest::digest(&ring::digest::SHA256, binding.as_bytes())
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let body = reserve.body.as_mut().unwrap();
    body["data"]["attributes"]["fileSize"] = SEARCH.len().into();
    body["data"]["attributes"]["referenceName"] = format!("parsequerade-{binding_sha}").into();
    steps.push(reserve);
    steps.push(Step::get("/upload", json!({})).put(SEARCH));
    steps.push(commit());
    steps.extend(placements(vec![]));
    steps.push(create_placement());
    steps.extend(placements(vec![placement("image1")]));
    for step in &mut steps {
        replace_type(&mut step.payload);
        if let Some(body) = &mut step.body {
            replace_type(body);
        }
        for value in step.query.values_mut() {
            if value == "PRODUCT_PAGE_HEADER_ASSET" {
                *value = "APP_STORE_SEARCH_RESULTS_ASSET".into();
            }
        }
    }
    let (client, task) = mock(steps).await;
    let receipt = client
        .upload_listing_image(
            &request(ListingImagePlacementType::SearchResults),
            SEARCH,
            &mut ListingImageUploadCheckpoint::default(),
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        receipt.placement_type,
        ListingImagePlacementType::SearchResults
    );
    task.await.unwrap();
}

#[tokio::test]
async fn reference_data_accessor_preserves_live_catalog_shape_without_writes() {
    let live: Value =
        serde_json::from_str(include_str!("fixtures/listing-reference-data.json")).unwrap();
    let (client, task) = mock(vec![Step::get("/v1/appAssetLibraryRefData", live.clone())]).await;
    assert_eq!(client.listing_image_reference_data().await.unwrap(), live);
    task.await.unwrap();
}

#[tokio::test]
async fn live_catalog_allows_header_and_search_uploads_to_platform_any_group() {
    for (placement_type, png, type_name) in [
        (
            ListingImagePlacementType::ProductPageHeader,
            HEADER,
            "PRODUCT_PAGE_HEADER_ASSET",
        ),
        (
            ListingImagePlacementType::SearchResults,
            SEARCH,
            "APP_STORE_SEARCH_RESULTS_ASSET",
        ),
    ] {
        let mut steps = preflight();
        steps.last_mut().unwrap().payload =
            serde_json::from_str(include_str!("fixtures/listing-reference-data.json")).unwrap();
        let mut reserve = reservation(vec![upload_operation("/upload", 0, png.len())]);
        let sha = |bytes: &[u8]| {
            ring::digest::digest(&ring::digest::SHA256, bytes)
                .as_ref()
                .iter()
                .map(|v| format!("{v:02x}"))
                .collect::<String>()
        };
        let binding = format!("app1:version1:en-US:{type_name}:listing.png:{}", sha(png));
        let body = reserve.body.as_mut().unwrap();
        body["data"]["attributes"]["fileSize"] = png.len().into();
        body["data"]["attributes"]["referenceName"] =
            format!("parsequerade-{}", sha(binding.as_bytes())).into();
        steps.push(reserve);
        steps.push(Step::get("/upload", json!({})).put(png));
        steps.push(commit());
        steps.extend(placements(vec![]));
        steps.push(create_placement());
        steps.extend(placements(vec![placement("image1")]));
        for step in &mut steps {
            if step.path == "/v1/appAssetLibraryRefData" {
                continue;
            }
            let adapt = |value: &Value| {
                serde_json::from_str::<Value>(
                    &value
                        .to_string()
                        .replace("CREATIVE_IOS", "DEFAULT_PROFILE")
                        .replace("PRODUCT_PAGE_HEADER_ASSET", type_name),
                )
                .unwrap()
            };
            step.payload = adapt(&step.payload);
            if let Some(body) = &mut step.body {
                *body = adapt(body);
            }
            if let Some(value) = step.query.get_mut("filter[placementType]") {
                *value = type_name.into();
            }
        }
        let (client, task) = mock(steps).await;
        let result = client
            .upload_listing_image(
                &request(placement_type),
                png,
                &mut ListingImageUploadCheckpoint::default(),
                |_| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(result.placement_type, placement_type);
        assert_eq!(result.placement_id, "placement1");
        task.await.unwrap();
    }
}

#[tokio::test]
async fn reserved_checkpoint_resumes_same_image_without_another_reservation() {
    let mut steps = preflight();
    steps.push(Step::get("/v1/appAssetLibraryImages/image1",single("appAssetLibraryImages","image1",json!({
        "state":"AWAITING_UPLOAD","uploadOperations":[upload_operation("/upload",0,HEADER.len())]
    }))));
    steps.push(Step::get("/upload", json!({})).put(HEADER));
    steps.push(commit());
    steps.extend(placements(vec![]));
    steps.push(create_placement());
    steps.extend(placements(vec![placement("image1")]));
    let (client, task) = mock(steps).await;
    let result = client
        .upload_listing_image(
            &request(ListingImagePlacementType::ProductPageHeader),
            HEADER,
            &mut state_checkpoint("reserved"),
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(result.image_id, "image1");
    assert_eq!(result.placement_id, "placement1");
    task.await.unwrap();
}
