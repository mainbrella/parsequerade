import { createRequire } from "node:module";

interface NativeCore {
  abiVersion(): number;
  clientNewJson(config: string): string;
  clientFree(client: number): void;
  startJson(client: number, request: string): string;
  pollJson(operation: number, timeoutMs: number): Promise<string>;
  respondJson(operation: number, event: number, response: string): boolean;
  cancel(operation: number): void;
  operationFree(operation: number): void;
}

// The build places exactly one platform's addon beside the bundled module.
export const core = createRequire(import.meta.url)("./parsequerade.node") as NativeCore;
if (core.abiVersion() !== 1) throw new Error("The Parsequerade native SDK ABI is incompatible. Rebuild the SDK.");
