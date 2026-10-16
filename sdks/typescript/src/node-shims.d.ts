// Only the Node APIs used by this package. Consumers do not need @types/node.
declare module "node:module" {
  export function createRequire(url: string): (id: string) => unknown;
}
declare module "node:buffer" {
  export const Buffer: {
    from(value: string | Uint8Array, encoding?: string): Uint8Array<ArrayBuffer> & {
      toString(encoding?: string): string;
    };
  };
}
