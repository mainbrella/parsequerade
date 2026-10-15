// Check the built package's public declarations with the same resolution consumers use.
import { Client, ApiError } from "@parsequerade/sdk";
import type { App, AppInfo, Version, VersionLocalization, AgeRating, AgeRatingUpdate, Page } from "@parsequerade/sdk";

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
  void [name, app, info, versions, localization, rating, updated, raw, wrongResult];
}
