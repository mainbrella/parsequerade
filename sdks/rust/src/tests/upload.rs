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
