import test from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const run = promisify(execFile);
const root = fileURLToPath(new URL("../../../", import.meta.url));
const sdk = fileURLToPath(new URL("../", import.meta.url));

test("packed SDK runs and typechecks outside the checkout without runtime dependencies", async () => {
  const directory = await mkdtemp(join(tmpdir(), "parsequerade-npm-"));
  try {
    // The enclosing test command already builds; pack the exact tested files.
    const packed = await run("npm", ["pack", sdk, "--ignore-scripts", "--json", "--pack-destination", directory], { cwd: directory });
    const [metadata] = JSON.parse(packed.stdout);
    assert.ok(metadata.files.some(file => file.path === "dist/index.d.ts"));
    assert.ok(metadata.files.some(file => file.path === "dist/index.js"));
    assert.ok(metadata.files.every(file => !file.path.startsWith("src/") && !file.path.startsWith("tests/")));
    await writeFile(join(directory, "package.json"), JSON.stringify({ private: true, type: "module" }));
    await run("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", join(directory, metadata.filename)], { cwd: directory });
    const installed = JSON.parse(await readFile(join(directory, "node_modules/@parsequerade/sdk/package.json"), "utf8"));
    assert.equal(Object.keys(installed.dependencies ?? {}).length, 0);
    await writeFile(join(directory, "consumer.mjs"), `
      import assert from 'node:assert/strict';
      import { generateKeyPairSync } from 'node:crypto';
      import { Client, ApiError } from '@parsequerade/sdk';
      const pair = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
      let calls = 0;
      const client = await Client.create({
        issuerId: '00000000-1111-2222-3333-444444444444', keyId: 'ABCDEF1234',
        privateKey: pair.privateKey.export({ type: 'pkcs8', format: 'pem' }),
        fetch: async (url, init) => {
          calls++;
          assert.equal(new URL(url).origin, 'https://api.appstoreconnect.apple.com');
          assert.match(init.headers.Authorization, /^Bearer /);
          return Response.json({ data: [{ type: 'apps', id: '123', attributes: { name: 'Packaged app' } }], links: {} });
        },
      });
      assert.equal(calls, 0);
      assert.equal((await client.apps()).items[0].name, 'Packaged app');
      assert.equal(calls, 1);
      await assert.rejects(client.apps({ limit: 0 }), error => error instanceof ApiError && error.code === 'invalid_query');
    `);
    await run(process.execPath, [join(directory, "consumer.mjs")], { cwd: directory });
    await writeFile(join(directory, "consumer.ts"), `
      import { Client, ApiError } from '@parsequerade/sdk';
      import type { App, Page, AgeRatingUpdate } from '@parsequerade/sdk';
      export async function consume(client: Client): Promise<Page<App>> {
        const update: AgeRatingUpdate = { gambling: false, kidsAgeBand: null };
        await client.updateAgeRating('123', 'info_1', update);
        try { return await client.apps({ fields: ['name', 'bundleId'] }); }
        catch (error) { if (error instanceof ApiError) console.log(error.code); throw error; }
      }
      // @ts-expect-error per-route enums are preserved in the installed declarations.
      const invalid: AgeRatingUpdate = { ageRatingOverrideV2: 'INVALID' };
    `);
    await writeFile(join(directory, "tsconfig.json"), JSON.stringify({ compilerOptions: {
      strict: true, noEmit: true, module: "NodeNext", moduleResolution: "NodeNext", target: "ES2023",
      types: [], lib: ["ES2023", "DOM", "DOM.Iterable"],
    }, include: ["consumer.ts"] }));
    await run(process.execPath, [join(root, "node_modules/typescript/bin/tsc"), "-p", join(directory, "tsconfig.json")], { cwd: directory });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
