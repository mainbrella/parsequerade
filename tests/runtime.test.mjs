import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile, rm, mkdtemp, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { Miniflare } from "miniflare";
import { unstable_getMiniflareWorkerOptions } from "wrangler";
import { makeEnv, request, app, json, jsonResponse, validateJwt, pem, connectionId } from "./helpers.mjs";

test("bundled Worker runs in workerd with the actual Wrangler asset routing", async t => {
  const { workerOptions, externalWorkers } = unstable_getMiniflareWorkerOptions("wrangler.jsonc");
  const env = makeEnv(); delete env.ASSETS;
  let calls = 0;
  const mf = new Miniflare({ workers: [{ ...workerOptions, modules: true,
    scriptPath: resolve(".wrangler/test-build/index.js"), bindings: env,
    outboundService: async incoming => {
      const url = new URL(incoming.url);
      assert.equal(url.origin, "https://api.appstoreconnect.apple.com"); validateJwt(incoming.headers.get("Authorization"));
      assert.equal(incoming.method, "GET"); calls++;
      return jsonResponse(json([app(calls === 1 ? "123" : "456")], calls === 1 ? "/v1/apps?cursor=next" : null));
    },
  }, ...externalWorkers] });
  t.after(() => mf.dispose());
  for (const [path, fragment] of [["/", "Parsequerade"], ["/docs/", "Introduction"], ["/docs/quickstart/", "Quickstart"]]) {
    const response = await mf.dispatchFetch("http://localhost" + path);
    assert.equal(response.status, 200); assert.match(await response.text(), new RegExp(fragment));
  }
  const spec = await mf.dispatchFetch("http://localhost/docs/openapi.json");
  assert.equal(spec.status, 200); assert.equal(Object.keys((await spec.json()).paths).length, 12);
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
});

test("generated .dev.vars loads through Wrangler and workerd without manual escaping", async t => {
  const directory = await mkdtemp(join(tmpdir(), "parsequerade-runtime-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, "AuthKey.p8"), pem, { mode: 0o600 });
  const result = spawnSync(process.execPath, ["scripts/configure.mjs", "--connection", connectionId, "--issuer-id", "00000000-1111-2222-3333-444444444444", "--key-id", "ABCDEF1234", "--private-key", join(directory, "AuthKey.p8"), "--app-id", "123", "--output-dir", directory], { encoding: "utf8" });
  assert.equal(result.status, 0);
  const bundle = await readFile(".wrangler/test-build/index.js", "utf8");
  await writeFile(join(directory, "index.js"), bundle);
  await writeFile(join(directory, "wrangler.jsonc"), JSON.stringify({ name: "parsequerade-setup-test", main: "./index.js", compatibility_date: "2026-09-13" }));
  const { workerOptions } = unstable_getMiniflareWorkerOptions(join(directory, "wrangler.jsonc"));
  const mf = new Miniflare({ ...workerOptions, modules: true, scriptPath: join(directory, "index.js"), outboundService: async incoming => {
    validateJwt(incoming.headers.get("Authorization")); return jsonResponse(json([app()]));
  } });
  t.after(() => mf.dispose());
  const key = (await readFile(join(directory, ".parsequerade-client-key"), "utf8")).trim();
  const response = await mf.dispatchFetch("http://localhost/v1/connections/conn_example/apps", { headers: { Authorization: `Bearer ${key}` } });
  assert.equal(response.status, 200); assert.equal((await response.json()).items[0].id, "123");
});
