#!/usr/bin/env node
import { parseArgs } from "node:util";
import { randomBytes, createHash, createPrivateKey } from "node:crypto";
import { readFile, writeFile, mkdir, access } from "node:fs/promises";
import { resolve, join } from "node:path";
import { authenticate, connectionConfig, identifier } from "../worker/auth.js";

const usage = `Usage: npm run configure -- --connection conn_main --issuer-id UUID --key-id KEYID12345 --private-key /path/AuthKey.p8 --app-id APPID
Repeat --app-id for several apps, or explicitly use --all-apps. Optional: --output-dir DIR.
Creates local configuration, deployment secrets, and a client key with mode 0600. Does not contact Apple or deploy.`;

try {
  const { values } = parseArgs({ options: {
    connection: { type: "string" }, "issuer-id": { type: "string" }, "key-id": { type: "string" },
    "private-key": { type: "string" }, "app-id": { type: "string", multiple: true },
    "all-apps": { type: "boolean" }, "output-dir": { type: "string", default: "." }, help: { type: "boolean" },
  } });
  if (values.help) { console.log(usage); process.exit(0); }
  if (!values.connection || !identifier.test(values.connection) || values.connection.startsWith("conn_live_") || !values["issuer-id"] || !values["key-id"] || !values["private-key"] ||
      (Boolean(values["all-apps"]) === Boolean(values["app-id"]?.length)) ||
      values["app-id"]?.some(id => !identifier.test(id)) || (values["app-id"]?.length || 0) > 200) throw new Error(usage);
  const privateKey = await readFile(values["private-key"], "utf8");
  let parsed;
  try { parsed = createPrivateKey(privateKey); } catch { throw new Error("Provide a valid PKCS#8 Apple .p8 private key."); }
  if (parsed.asymmetricKeyType !== "ec" || parsed.asymmetricKeyDetails.namedCurve !== "prime256v1" || !privateKey.startsWith("-----BEGIN PRIVATE KEY-----")) {
    throw new Error("The private key must be PKCS#8 using the P-256 curve.");
  }
  const clientKey = `pq_${randomBytes(32).toString("base64url")}`;
  const secrets = {
    APPLE_CONNECTIONS_JSON: JSON.stringify({ [values.connection]: { issuerId: values["issuer-id"], keyId: values["key-id"],
      credentialRevision: "1", privateKeySecret: "APPLE_PRIVATE_KEY" } }),
    APPLE_PRIVATE_KEY: privateKey,
    FACADE_API_KEYS_JSON: JSON.stringify([{ id: "client_main", sha256: createHash("sha256").update(clientKey).digest("hex"),
      grants: [{ connectionId: values.connection, capabilities: ["read"], appIds: values["all-apps"] ? "*" : [...new Set(values["app-id"])] }] }]),
    CURSOR_SIGNING_KEY: randomBytes(32).toString("base64url"),
  };
  try {
    connectionConfig(secrets, values.connection);
    await authenticate(new Request("https://configuration.local", { headers: { Authorization: `Bearer ${clientKey}` } }), secrets, values.connection);
  } catch { throw new Error("Check the issuer UUID, 10-character Apple key ID, connection identifier, and configuration size (5 KiB per secret)."); }
  const directory = resolve(values["output-dir"]);
  const targets = [".dev.vars", ".parsequerade.secrets.json", ".parsequerade-client-key"].map(name => join(directory, name));
  for (const target of targets) {
    const exists = await access(target).then(() => true, () => false);
    if (exists) throw new Error("Configuration files already exist in the output directory. Choose a new directory or edit them explicitly.");
  }
  await mkdir(directory, { recursive: true });
  const local = Object.entries(secrets).map(([name, value]) => name === "APPLE_PRIVATE_KEY" ? `${name}=${JSON.stringify(value)}` : `${name}='${value}'`).join("\n") + "\n";
  await writeFile(targets[0], local, { flag: "wx", mode: 0o600 });
  await writeFile(targets[1], JSON.stringify(secrets, null, 2) + "\n", { flag: "wx", mode: 0o600 });
  await writeFile(targets[2], clientKey + "\n", { flag: "wx", mode: 0o600 });
  console.log("Created .dev.vars, .parsequerade.secrets.json, and .parsequerade-client-key in the output directory. Keep all three private. No deployment was made.");
} catch (error) {
  // Native parsing errors can include input text; emit only our own configuration guidance.
  console.error(error instanceof Error && (error.message.startsWith("Usage:") || error.message.startsWith("Configuration files") ||
    error.message.startsWith("The private key") || error.message.startsWith("Provide a valid") || error.message.startsWith("Check the issuer")) ?
    error.message : "Configuration failed. Check the arguments, private-key file, and output-directory permissions. Use --help for usage.");
  process.exitCode = 1;
}
