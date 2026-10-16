// Share the Worker's read fixtures with native SDK tests.
import { readFixture } from '../tests/helpers.mjs';
import { routes } from '../worker/resources.js';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
const fixtures = JSON.stringify(routes.map(route => ({ handler: route.handler, collection: route.collection, ...readFixture(route) })), null, 2) + '\n';
const scenarios = await readFile(new URL('../tests/sdk/scenarios.json', import.meta.url), 'utf8');
const outputs = new Map([
  ['../sdks/swift/Tests/ParsequeradeTests/Fixtures/reads.json', fixtures],
  ['../sdks/go/testdata/reads.json', fixtures],
  ['../sdks/rust/src/tests/scenarios.json', scenarios],
]);
for (const [relative, content] of outputs) {
  const path = new URL(relative, import.meta.url);
  if (process.argv.includes('--check')) {
    if (await readFile(path, 'utf8') !== content) throw new Error('Native SDK fixtures are stale; run npm run generate:sdks');
  } else {
    await mkdir(new URL('.', path), { recursive: true });
    await writeFile(path, content);
  }
}
