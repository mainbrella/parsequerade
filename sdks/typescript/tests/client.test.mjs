import test from "node:test";
import assert from "node:assert/strict";
import { inspect } from "node:util";
import { Client, ApiError } from "../dist/index.js";
import { routes } from "../../../worker/resources.js";
import { pem, app, appInfo, version, localization, resource, json, jsonResponse, validateJwt, readFixture } from "../../../tests/helpers.mjs";

const credentials = { issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem };

async function fixture(responses = []) {
  const calls = [];
  const client = await Client.create({ ...credentials, fetch: async (url, init) => {
    const call = { url: new URL(url), init };
    calls.push(call);
    assert.equal(call.url.origin, "https://api.appstoreconnect.apple.com");
    assert.equal(init.redirect, "manual");
    validateJwt(init.headers.Authorization);
    const response = responses[calls.length - 1];
    assert.ok(response, `Unexpected Apple request ${call.url.pathname}`);
    if (typeof response === "function") return response(call);
    return response instanceof Response ? response : jsonResponse(response);
  } });
  return { client, calls };
}

function error(code, status) {
  return caught => {
    assert.ok(caught instanceof ApiError);
    assert.equal(caught.code, code);
    if (status) assert.equal(caught.status, status);
    return true;
  };
}

test("all twelve typed methods use the Worker fixtures and flat resource contract", async t => {
  for (const route of routes) await t.test(route.handler, async () => {
    const { item, responses } = readFixture(route);
    const f = await fixture(responses);
    assert.equal(f.calls.length, 0, "construction makes no probe request");
    const params = [...route.route.matchAll(/\{(\w+)\}/g)].map(([, name]) => ({ appId: "123", appInfoId: "info_1", versionId: "version_1", locale: "en-US" })[name]);
    const body = await f.client[route.handler](...params);
    const value = route.collection ? body.items[0] : body;
    assert.equal(value.id, item.id);
    assert.equal(value.attributes, undefined);
    assert.equal(value.relationships, undefined);
    assert.equal(value.notes, undefined);
    assert.equal(value.demoAccountPassword, undefined);
    if (!["apps", "app"].includes(route.handler)) assert.equal(value.appId, "123");
    if (route.collection) assert.equal(body.nextCursor, null);
    if (route.handler === "ageRating") {
      assert.equal(value.gambling, false);
      assert.equal(value.ageRatingOverrideV2, "FUTURE_READ_VALUE");
    }
    assert.equal(f.calls.length, responses.length);
    assert.ok(f.calls.every(call => call.init.method === "GET"));
  });
});

test("object and URLSearchParams queries translate without losing false or null", async () => {
  const f = await fixture([
    json([app("123", { isOrEverWasMadeForKids: false, subscriptionStatusUrl: null })]),
    json(app()),
    json([version()]),
  ]);
  const result = await f.client.apps({ bundleId: ["com.example.notes", "com.example.other"], fields: ["name", "isOrEverWasMadeForKids", "subscriptionStatusUrl"], sort: "-name", limit: 2 });
  assert.equal(result.items[0].isOrEverWasMadeForKids, false);
  assert.equal(result.items[0].subscriptionStatusUrl, null);
  assert.equal(result.items[0].bundleId, undefined);
  assert.equal(f.calls[0].url.searchParams.get("filter[bundleId]"), "com.example.notes,com.example.other");
  assert.equal(f.calls[0].url.searchParams.get("fields[apps]"), "name,isOrEverWasMadeForKids,subscriptionStatusUrl");
  assert.equal(f.calls[0].url.searchParams.get("sort"), "-name");
  await f.client.get("/apps/123", new URLSearchParams({ fields: "name" }));
  await f.client.versions("123", { platform: ["IOS", "MAC_OS"], state: "PREPARE_FOR_SUBMISSION" });
  assert.equal(f.calls[2].url.searchParams.get("filter[platform]"), "IOS,MAC_OS");
  assert.equal(f.calls[2].url.searchParams.get("filter[appVersionState]"), "PREPARE_FOR_SUBMISSION");
  assert.ok(f.calls.every(call => call.init.headers.Authorization === f.calls[0].init.headers.Authorization), "JWT reused across requests");
});

test("opaque pagination preserves filters and binds cursors to client, path and query", async () => {
  const f = await fixture([
    json([app()], "/v1/apps?cursor=apple_next"),
    json([app("456")]),
  ]);
  const query = { bundleId: "com.example.notes", fields: ["name"], limit: 1 };
  const first = await f.client.apps(query);
  assert.ok(first.nextCursor);
  await assert.rejects(f.client.apps({ ...query, limit: 2, cursor: first.nextCursor }), error("invalid_cursor"));
  await assert.rejects(f.client.apps({ ...query, cursor: first.nextCursor + "a" }), error("invalid_cursor"));
  const other = await fixture();
  await assert.rejects(other.client.apps({ ...query, cursor: first.nextCursor }), error("invalid_cursor"));
  assert.equal(other.calls.length, 0);
  const second = await f.client.apps({ ...query, cursor: first.nextCursor });
  assert.equal(second.items[0].id, "456");
  assert.equal(second.nextCursor, null);
  assert.equal(f.calls[1].url.searchParams.get("cursor"), "apple_next");
  assert.equal(f.calls[1].url.searchParams.get("filter[bundleId]"), query.bundleId);
  assert.equal(f.calls[1].url.searchParams.get("fields[apps]"), "name");
  assert.equal(f.calls.length, 2);
});

test("SDK uses the same ownership checks, exact locale selection and category projection", async () => {
  const f = await fixture([
    json([version()]), json([localization()]), json(localization("en-US", "locale_1", { keywords: "hidden" })),
    json([appInfo()]),
    json(resource("appInfos", "info_1", { state: "LIVE" }, {
      primaryCategory: { data: { type: "appCategories", id: "UTILITIES" } }, secondaryCategory: { data: null },
    })),
  ]);
  assert.deepEqual(await f.client.versionLocalization("123", "version_1", "en-US", { fields: ["description"] }), {
    id: "locale_1", appId: "123", versionId: "version_1", description: "Example description",
  });
  assert.equal(f.calls[0].url.pathname, "/v1/apps/123/appStoreVersions");
  assert.equal(f.calls[2].url.searchParams.get("fields[appStoreVersionLocalizations]"), "description,locale");
  const info = await f.client.appInfo("123", "info_1", { fields: "state" });
  assert.equal(info.primaryCategoryId, "UTILITIES");
  assert.equal(info.secondaryCategoryId, null);
  assert.equal(f.calls[4].url.searchParams.get("include"), "primaryCategory,secondaryCategory");
  assert.equal(f.calls[4].url.searchParams.get("fields[appInfos]"), "state,primaryCategory,secondaryCategory");

  const missing = await fixture([json([])]);
  await assert.rejects(missing.client.ageRating("123", "info_1"), error("resource_not_found", 404));
  assert.equal(missing.calls.length, 1);
  const ambiguous = await fixture([
    json([version()]), json([localization()], "?cursor=more"), json([localization("en-US", "another")]),
  ]);
  await assert.rejects(ambiguous.client.versionLocalization("123", "version_1", "en-US"), error("ambiguous_selection", 409));
});

test("age-rating update resolves scope, snapshots attributes and sends one typed PATCH", async () => {
  const attributes = { gambling: false, kidsAgeBand: null, gracRatingClassificationNumber: "" };
  const expected = structuredClone(attributes);
  const f = await fixture([
    () => { attributes.gambling = true; return jsonResponse(json([appInfo()])); },
    json(resource("ageRatingDeclarations", "rating_1", { gambling: true })),
    call => {
      assert.equal(call.init.method, "PATCH");
      assert.equal(call.url.pathname, "/v1/ageRatingDeclarations/rating_1");
      assert.deepEqual(JSON.parse(call.init.body), { data: { type: "ageRatingDeclarations", id: "rating_1", attributes: expected } });
      return jsonResponse(json(resource("ageRatingDeclarations", "rating_1", expected)));
    },
  ]);
  const result = await f.client.updateAgeRating("123", "info_1", attributes);
  assert.equal(result.gambling, false);
  assert.equal(result.kidsAgeBand, null);
  assert.equal(result.gracRatingClassificationNumber, "");
  assert.equal(result.appInfoId, "info_1");
  assert.equal(f.calls.length, 3);
});

test("Apple write errors and uncertain outcomes are never retried", async t => {
  const results = [
    ...[401, 403, 409, 422, 429, 503].map(status => [String(status), jsonResponse({ errors: [{ code: "APPLE_ERROR", detail: "private-value" }] }, status)]),
    ["network", () => { throw new Error("private-value"); }],
    ["malformed", jsonResponse({ unexpected: "private-value" })],
    ["wrong resource", jsonResponse(json(resource("ageRatingDeclarations", "wrong_id")))],
  ];
  for (const [name, response] of results) await t.test(name, async () => {
    const f = await fixture([json([appInfo()]), json(resource("ageRatingDeclarations", "rating_1")), response]);
    await assert.rejects(f.client.updateAgeRating("123", "info_1", { gambling: false }), caught => {
      assert.ok(caught instanceof ApiError);
      assert.equal(caught.retryable, false);
      assert.equal(inspect(caught).includes("private-value"), false);
      if (["network", "malformed", "wrong resource"].includes(name)) assert.equal(caught.code, "write_outcome_unknown");
      return true;
    });
    assert.equal(f.calls.length, 3);
  });
});

test("read authentication refresh, throttles and error details use shared transport behavior", async () => {
  const f = await fixture([jsonResponse({}, 401), json(app())]);
  await f.client.app("123");
  assert.equal(f.calls.length, 2);
  assert.notEqual(f.calls[0].init.headers.Authorization, f.calls[1].init.headers.Authorization);
  const throttle = await fixture([jsonResponse({ errors: [{ code: "THROTTLED", detail: "private", source: { parameter: "filter[bundleId]" } }] }, 429, { "Retry-After": "30" })]);
  await assert.rejects(throttle.client.apps({ bundleId: "com.example.notes" }), caught => {
    assert.ok(caught instanceof ApiError);
    assert.equal(caught.headers["Retry-After"], "30");
    assert.equal(caught.details[0].field, "bundleId");
    assert.equal(caught.retryable, true);
    assert.equal(caught.message.includes("private"), false);
    return true;
  });
  assert.equal(throttle.calls.length, 1);
});

test("invalid credentials, queries, resource identifiers and update attributes fail locally", async () => {
  const f = await fixture();
  assert.equal(inspect(f.client).includes("PRIVATE KEY"), false);
  for (const options of [{ ...credentials, privateKey: "secret" }, { ...credentials, issuerId: "bad" }, { ...credentials, keyId: "bad" }, { ...credentials, fetch: "bad" }]) {
    await assert.rejects(Client.create(options), error("invalid_credentials", 400));
  }
  for (const query of [{ limit: 0 }, { limit: 201 }, { fields: "unknown" }, { sort: ["name", "unknown"] }, { include: "versions" }, { limit: null }, { fields: {} }, { fields: ["name", 2] }, new URLSearchParams("limit=1&limit=2")]) {
    await assert.rejects(f.client.apps(query), error("invalid_query", 400));
  }
  for (const id of ["../other", "x/y", "x?query", "", "%2F", 123]) await assert.rejects(f.client.app(id), error("invalid_path", 400));
  for (const attributes of [{}, [], null, { id: "rating_1" }, { gambling: null }, { gambling: "false" }, { kidsAgeBand: "INVALID" }, { ageRatingOverride: "NONE" }, { developerAgeRatingInfoUrl: "https://user:pass@example.com" }]) {
    await assert.rejects(f.client.updateAgeRating("123", "info_1", attributes), error("invalid_request", 400));
  }
  await assert.rejects(f.client.updateAgeRating("123", "info_1", { gracRatingClassificationNumber: "x".repeat(9000) }), error("request_too_large", 413));
  await assert.rejects(f.client.get("https://other.example/apps"), error("route_not_found", 404));
  await assert.rejects(f.client.get("/sales-reports"), error("route_not_found", 404));
  assert.equal(f.calls.length, 0);
});

test("pagination rejects untrusted links and repeated pages", async () => {
  for (const next of ["https://other.example/v1/apps?cursor=x", "/v1/other?cursor=x", "?cursor=x&limit=200", "?cursor=x&cursor=y"]) {
    const f = await fixture([json([], next)]);
    await assert.rejects(f.client.apps(), error("upstream_invalid_response", 502));
    assert.equal(f.calls.length, 1);
  }
  const f = await fixture([json([], "?cursor=repeat"), json([], "?cursor=repeat")]);
  const first = await f.client.apps();
  await assert.rejects(f.client.apps({ cursor: first.nextCursor }), error("upstream_incomplete_result", 502));
  assert.equal(f.calls.length, 2);
});
