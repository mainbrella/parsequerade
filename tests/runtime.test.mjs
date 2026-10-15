import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile, rm, mkdtemp, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { Miniflare, convertV4MiniflareOptions } from "miniflare";
import { unstable_getMiniflareWorkerOptions } from "wrangler";
import { makeEnv, request, app, json, jsonResponse, validateJwt, pem, connectionId, appInfo, resource } from "./helpers.mjs";

test("bundled Worker runs in workerd with the actual Wrangler asset routing", async t => {
  const { workerOptions, externalWorkers } = unstable_getMiniflareWorkerOptions("wrangler.jsonc");
  // The dry-run bundle has no external modules, so Wrangler's source module rules are unnecessary.
  delete workerOptions.modulesRules;
  const env = makeEnv(); delete env.ASSETS;
  let calls = 0;
  const mf = new Miniflare(convertV4MiniflareOptions({ workers: [{ ...workerOptions, modules: true,
    scriptPath: resolve(".wrangler/test-build/entry.js"), bindings: env,
    outboundService: async incoming => {
      const url = new URL(incoming.url);
      assert.equal(url.origin, "https://api.appstoreconnect.apple.com"); validateJwt(incoming.headers.get("Authorization"));
      assert.equal(incoming.method, "GET"); calls++;
      return jsonResponse(json([app(calls === 1 ? "123" : "456")], calls === 1 ? "/v1/apps?cursor=next" : null));
    },
  }, ...externalWorkers] }));
  t.after(() => mf.dispose());
  for (const [path, fragment] of [["/", "Parsequerade"], ["/docs/", "Introduction"], ["/docs/quickstart/", "Quickstart"]]) {
    const response = await mf.dispatchFetch("http://localhost" + path);
    assert.equal(response.status, 200); assert.match(await response.text(), new RegExp(fragment));
  }
  const spec = await mf.dispatchFetch("http://localhost/docs/openapi.json");
  assert.equal(spec.status, 200); assert.equal(Object.keys((await spec.json()).paths).length, 14);
  const unauthorized = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps");
  assert.equal(unauthorized.status, 401); assert.equal((await unauthorized.json()).error.code, "unauthorized");
  const headers = Object.fromEntries(request().headers);
  const first = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps?limit=1", { headers });
  assert.equal(first.status, 200); const body = await first.json(); assert.equal(body.items[0].name, "Example Notes");
  const second = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps?limit=1&cursor=" + encodeURIComponent(body.nextCursor), { headers });
  assert.equal(second.status, 200); assert.equal((await second.json()).nextCursor, null); assert.equal(calls, 2);
  const write = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps/123", { method: "PATCH", headers, body: "{}" });
  assert.equal(write.status, 405); assert.equal(calls, 2);
  const missingApi = await mf.dispatchFetch("http://localhost/v1/not-implemented");
  assert.equal(missingApi.status, 404); assert.match(missingApi.headers.get("Content-Type"), /application\/json/);
  const missingSite = await mf.dispatchFetch("http://localhost/missing-page");
  assert.equal(missingSite.status, 404); assert.match(missingSite.headers.get("Content-Type"), /text\/html/);
  for (const path of ["/.index.html.swp", "/.assetsignore", "/worker/index.js", "/.parsequerade.secrets.json"]) {
    assert.equal((await mf.dispatchFetch("http://localhost" + path)).status, 404);
  }
});

test("hosted connections survive runtime restarts and revoke keys in workerd", async t => {
  const directory = await mkdtemp(join(tmpdir(), "parsequerade-hosted-runtime-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const { workerOptions, externalWorkers } = unstable_getMiniflareWorkerOptions("wrangler.jsonc");
  delete workerOptions.modulesRules;
  let calls = 0;
  const options = convertV4MiniflareOptions({ workers: [{ ...workerOptions, modules: true,
    scriptPath: resolve(".wrangler/test-build/entry.js"), bindings: { CONNECTION_ENCRYPTION_KEY: randomBytes(32).toString("base64url") },
    outboundService: async incoming => {
      assert.equal(new URL(incoming.url).origin, "https://api.appstoreconnect.apple.com");
      assert.equal(incoming.method, "GET"); validateJwt(incoming.headers.get("Authorization")); calls++;
      return jsonResponse(json([app()]));
    },
  }, ...externalWorkers] });
  options.resourcePersistencePath = directory;
  let mf = new Miniflare(options);
  t.after(() => mf.dispose());
  const created = await mf.dispatchFetch("http://localhost/v1/connections", { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem }) });
  assert.equal(created.status, 201, await created.clone().text());
  const connection = await created.json();
  assert.equal(created.headers.get("Location"), connection.baseUrl);
  assert.match(connection.connectionId, /^conn_live_[a-f0-9]{32}$/);
  assert.match(connection.apiKey, /^pq_[A-Za-z0-9_-]{43}$/);
  assert.equal(calls, 1);
  const headers = { Authorization: `Bearer ${connection.apiKey}` };
  await mf.dispose();
  mf = new Miniflare(options);
  const read = await mf.dispatchFetch(connection.baseUrl + "/apps", { headers });
  assert.equal(read.status, 200, await read.clone().text());
  assert.equal((await read.json()).items[0].id, "123"); assert.equal(calls, 2);
  const write = await mf.dispatchFetch(connection.baseUrl + "/apps/123", { method: "PATCH", headers, body: "{}" });
  assert.equal(write.status, 405); assert.equal(calls, 2);
  const removed = await mf.dispatchFetch(connection.baseUrl, { method: "DELETE", headers });
  assert.equal(removed.status, 204); assert.equal(await removed.text(), "");
  const revoked = await mf.dispatchFetch(connection.baseUrl + "/apps", { headers });
  assert.equal(revoked.status, 401); assert.equal(calls, 2);
});

test("generated .dev.vars loads through Wrangler and workerd without manual escaping", async t => {
  const directory = await mkdtemp(join(tmpdir(), "parsequerade-runtime-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, "AuthKey.p8"), pem, { mode: 0o600 });
  const result = spawnSync(process.execPath, ["scripts/configure.mjs", "--connection", connectionId, "--issuer-id", "00000000-1111-2222-3333-444444444444", "--key-id", "ABCDEF1234", "--private-key", join(directory, "AuthKey.p8"), "--app-id", "123", "--output-dir", directory], { encoding: "utf8" });
  assert.equal(result.status, 0);
  const bundle = await readFile(".wrangler/test-build/entry.js", "utf8");
  await writeFile(join(directory, "index.js"), bundle);
  await writeFile(join(directory, "wrangler.jsonc"), JSON.stringify({ name: "parsequerade-setup-test", main: "./index.js", compatibility_date: "2026-09-13" }));
  const { workerOptions } = unstable_getMiniflareWorkerOptions(join(directory, "wrangler.jsonc"));
  delete workerOptions.modulesRules;
  const mf = new Miniflare(convertV4MiniflareOptions({ ...workerOptions, modules: true, scriptPath: join(directory, "index.js"), outboundService: async incoming => {
    validateJwt(incoming.headers.get("Authorization")); return jsonResponse(json([app()]));
  } }));
  t.after(() => mf.dispose());
  const key = (await readFile(join(directory, ".parsequerade-client-key"), "utf8")).trim();
  const response = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps", { headers: { Authorization: `Bearer ${key}` } });
  assert.equal(response.status, 200); assert.equal((await response.json()).items[0].id, "123");
});


test("scoped age-rating POST runs in workerd and patches only the target declaration", async t => {
  const { workerOptions, externalWorkers } = unstable_getMiniflareWorkerOptions("wrangler.jsonc");
  delete workerOptions.modulesRules;
  const env = makeEnv(); delete env.ASSETS;
  const keys = JSON.parse(env.FACADE_API_KEYS_JSON); keys[0].grants[0].capabilities = ["read", "write"]; env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
  const methods = [];
  const mf = new Miniflare(convertV4MiniflareOptions({ workers: [{ ...workerOptions, modules: true,
    scriptPath: resolve(".wrangler/test-build/entry.js"), bindings: env,
    outboundService: async incoming => {
      const url = new URL(incoming.url); methods.push(incoming.method); validateJwt(incoming.headers.get("Authorization"));
      assert.equal(url.origin, "https://api.appstoreconnect.apple.com");
      if (incoming.method === "PATCH") {
        assert.equal(url.pathname, "/v1/ageRatingDeclarations/target_rating");
        const body = await incoming.json();
        assert.deepEqual(body, { data: { type: "ageRatingDeclarations", id: "target_rating", attributes: { gambling: false } } });
        return jsonResponse(json(resource("ageRatingDeclarations", "target_rating", body.data.attributes)));
      }
      assert.equal(incoming.method, "GET");
      if (url.pathname === "/v1/apps/123/appInfos") return jsonResponse(json([appInfo()]));
      assert.equal(url.pathname, "/v1/appInfos/info_1/ageRatingDeclaration");
      return jsonResponse(json(resource("ageRatingDeclarations", "target_rating", { gambling: true })));
    },
  }, ...externalWorkers] }));
  t.after(() => mf.dispose());
  const response = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps/123/app-infos/info_1/age-rating", {
    method: "POST", headers: { ...Object.fromEntries(request().headers), "Content-Type": "application/json" }, body: '{"gambling":false}',
  });
  assert.equal(response.status, 200); assert.deepEqual(await response.json(), { id: "target_rating", appId: "123", appInfoId: "info_1", gambling: false });
  assert.deepEqual(methods, ["GET", "GET", "PATCH"]);
});
