// Check the built package's public declarations with the same resolution consumers use.
import { Client, ApiError } from "@parsequerade/sdk";
import type { App, AppInfo, Version, VersionLocalization, AgeRating, AgeRatingUpdate, Page,
  Screenshot, Preview, Build, ListingImageUploadCheckpoint, ListingImageUploadReceipt } from "@parsequerade/sdk";

export async function checkTypes(client: Client): Promise<void> {
  const apps: Page<App> = await client.apps({ fields: ["name", "bundleId"], limit: 50, sort: "-name" });
  const name: string | null | undefined = apps.items[0]?.name;
  const app: App = await client.app("123", { fields: "name" });
  const info: AppInfo = await client.appInfo("123", "info_1");
  const versions: Page<Version> = await client.versions("123", { platform: ["IOS", "MAC_OS"], state: "PREPARE_FOR_SUBMISSION" });
  const localization: VersionLocalization = await client.versionLocalization("123", "v1", "en-US", { fields: "description" });
  const rating: AgeRating = await client.ageRating("123", "info_1");
  const update: AgeRatingUpdate = { gambling: false, kidsAgeBand: null, ageRatingOverrideV2: "NONE" };
  const updated: AgeRating = await client.updateAgeRating("123", "info_1", update);
  await client.apps(new URLSearchParams({ fields: "name,bundleId" }));
  const raw: App = await client.get<App>("/apps/123", { fields: ["name", "bundleId"] });
  const screenshots: Page<Screenshot> = await client.screenshots("set1");
  const width: number | null | undefined = screenshots.items[0]?.imageAsset?.width;
  const previews: Page<Preview> = await client.previews("set1");
  const offset: number | null | undefined = previews.items[0]?.uploadOperations?.[0]?.offset;
  const warning: string | null | undefined = previews.items[0]?.videoDeliveryState?.warnings?.[0]?.code;
  const builds: Page<Build> = await client.builds("123", { platform: "IOS", prereleaseVersion: "2.0" });
  const created: Version = await client.createVersion("123", "IOS", "2.0");
  const attached: Version = await client.attachBuild("123", created.id, builds.items[0].id);
  await client.updateMetadata("/apps/123/versions/v1", { copyright: "2026 Example" });
  const checkpoint: ListingImageUploadCheckpoint = {};
  const receipt: ListingImageUploadReceipt = await client.uploadListingImage({
    appId: "123", versionId: "v1", locale: "en-US", fileName: "listing.png",
    placementType: "PRODUCT_PAGE_HEADER_ASSET",
  }, new Uint8Array(), checkpoint, async snapshot => { console.log(snapshot.phase); });

  // @ts-expect-error apps() returns a page, not an app.
  const wrongResult: App = await client.apps();
  // @ts-expect-error fields are restricted to the route's attributes.
  await client.apps({ fields: ["description"] });
  // @ts-expect-error platform must be a supported enum value.
  await client.versions("123", { platform: "ANDROID" });
  // @ts-expect-error state must be a supported filter value.
  await client.versions("123", { state: "FUTURE_WRITE_VALUE" });
  // @ts-expect-error single-resource reads do not accept pagination.
  await client.app("123", { limit: 1 });
  // @ts-expect-error unsupported query names fail at compile time.
  await client.apps({ include: "appInfos" });
  // @ts-expect-error path parameters are strings.
  await client.app(123);
  // @ts-expect-error questionnaire answers cannot be cleared with null.
  await client.updateAgeRating("123", "info_1", { gambling: null });
  // @ts-expect-error writes reject deprecated fields.
  await client.updateAgeRating("123", "info_1", { ageRatingOverride: "NONE" });
  // @ts-expect-error override values are checked at compile time.
  await client.updateAgeRating("123", "info_1", { ageRatingOverrideV2: "INVALID" });
  // @ts-expect-error secret credentials are private.
  client.state;
  // @ts-expect-error media arrays are arrays, not query selections.
  const invalidOperations: Preview = { id: "p1", uploadOperations: "PUT" };
  // @ts-expect-error unsupported build platform is rejected in queries.
  await client.builds("123", { platform: "ANDROID" });
  // @ts-expect-error unsupported creation platform is rejected.
  await client.createVersion("123", "ANDROID", "2.0");
  // @ts-expect-error unsupported metadata field is rejected.
  await client.updateMetadata("/apps/123", { secret: "value" });
  // @ts-expect-error upload persistence must resolve without a return value.
  await client.uploadListingImage({ appId: "123", versionId: "v1", locale: "en-US", fileName: "x.png", placementType: "PRODUCT_PAGE_HEADER_ASSET" }, new Uint8Array(), {}, async () => 1);

  try { await client.apps(); }
  catch (error) {
    if (error instanceof ApiError) {
      const code: string = error.code;
      const retryable: boolean = error.retryable;
      const status: number = error.status;
      const field: string | undefined = error.details[0]?.field;
      void [code, retryable, status, field];
    }
  }
  void [name, app, info, versions, localization, rating, updated, raw, wrongResult, width, offset, warning, attached, receipt, invalidOperations];
}
