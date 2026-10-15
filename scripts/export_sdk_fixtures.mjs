// Share the Worker's read fixtures with native SDK tests.
import { readFixture } from '../tests/helpers.mjs';
import { routes } from '../worker/resources.js';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
const fixtures = JSON.stringify(routes.map(route => ({ handler: route.handler, collection: route.collection, ...readFixture(route) })), null, 2) + '\n';
const path = new URL('../sdks/swift/Tests/ParsequeradeTests/Fixtures/reads.json', import.meta.url);
if (process.argv.includes('--check')) {
  if (await readFile(path, 'utf8') !== fixtures) throw new Error('Native SDK fixtures are stale; run npm run generate:sdks');
} else {
  await mkdir(new URL('.', path), { recursive: true });
  await writeFile(path, fixtures);
}
