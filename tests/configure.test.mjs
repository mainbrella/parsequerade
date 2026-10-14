import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, stat, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { pem, connectionId } from "./helpers.mjs";
import { authenticate } from "../worker/auth.js";

export async function configuredFixture() {
  const directory = await mkdtemp(join(tmpdir(), "parsequerade-test-"));
  const keyPath = join(directory, "AuthKey.p8");
  await writeFile(keyPath, pem, { mode: 0o600 });
  const args = [resolve("scripts/configure.mjs"), "--connection", connectionId, "--issuer-id", "00000000-1111-2222-3333-444444444444", "--key-id", "ABCDEF1234", "--private-key", keyPath, "--app-id", "123", "--output-dir", directory];
  const result = spawnSync(process.execPath, args, { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  const env = JSON.parse(await readFile(join(directory, ".parsequerade.secrets.json"), "utf8"));
  const clientKey = (await readFile(join(directory, ".parsequerade-client-key"), "utf8")).trim();
  return { directory, args, result, env, clientKey };
}

test("setup generates scoped, usable secrets and a private client key without printing secrets", async () => {
  const fixture = await configuredFixture();
  try {
    const { directory, env, clientKey, result } = fixture;
    const records = JSON.parse(env.FACADE_API_KEYS_JSON);
    assert.equal(records[0].sha256, createHash("sha256").update(clientKey).digest("hex"));
    assert.equal(JSON.stringify(env).includes(clientKey), false);
    assert.equal(result.stdout.includes(clientKey), false); assert.equal(result.stdout.includes(pem), false);
    for (const path of [".dev.vars", ".parsequerade.secrets.json", ".parsequerade-client-key"]) assert.equal((await stat(join(directory, path))).mode & 0o777, 0o600);
    const auth = await authenticate(new Request("https://facade.example", { headers: { Authorization: `Bearer ${clientKey}` } }), env, connectionId);
    assert.deepEqual(auth.grant.appIds, ["123"]); assert.deepEqual(auth.grant.capabilities, ["read"]);
    const second = spawnSync(process.execPath, fixture.args, { encoding: "utf8" });
    assert.equal(second.status, 1); assert.match(second.stderr, /already exist/);
    assert.equal((await readFile(join(directory, ".parsequerade-client-key"), "utf8")).trim(), clientKey);
  } finally { await rm(fixture.directory, { recursive: true, force: true }); }
});

test("setup requires an explicit app scope", () => {
  const result = spawnSync(process.execPath, ["scripts/configure.mjs", "--connection", "conn_example"], { encoding: "utf8" });
  assert.equal(result.status, 1); assert.match(result.stderr, /--all-apps/);
});
