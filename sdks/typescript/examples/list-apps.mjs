import { readFile } from "node:fs/promises";
import { Client } from "@parsequerade/sdk";

const client = await Client.create({
  issuerId: process.env.ASC_ISSUER_ID,
  keyId: process.env.ASC_KEY_ID,
  privateKey: await readFile(process.env.ASC_PRIVATE_KEY_PATH, "utf8"),
});

let cursor = null;
do {
  const page = await client.apps({ limit: 50, ...(cursor ? { cursor } : {}) });
  for (const app of page.items) {
    console.log(`${app.id}\t${app.name ?? "(unnamed)"}`);
  }
  cursor = page.nextCursor;
} while (cursor);
