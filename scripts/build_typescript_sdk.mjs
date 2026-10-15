import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const sdk = resolve(root, "sdks/typescript");
const output = join(sdk, "dist");
const temporary = await mkdtemp(join(tmpdir(), "parsequerade-types-"));

try {
  execFileSync(process.execPath, [join(root, "node_modules/typescript/bin/tsc"),
    "-p", join(sdk, "tsconfig.json"), "--outDir", temporary], { stdio: "inherit" });
  await mkdir(output, { recursive: true });
  const declarations = join(temporary, "sdks/typescript/src");
  for (const file of await readdir(declarations)) {
    await copyFile(join(declarations, file), join(output, file));
  }
  await build({
    absWorkingDir: root,
    entryPoints: [join(sdk, "src/index.ts")],
    outfile: join(output, "index.js"),
    bundle: true, format: "esm", platform: "neutral", target: "es2023",
    logLevel: "info",
  });
} finally {
  await rm(temporary, { recursive: true, force: true });
}
