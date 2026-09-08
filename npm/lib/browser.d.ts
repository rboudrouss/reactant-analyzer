// The browser entry point — the same API as Node's, minus the calls that
// need a filesystem, plus control over where the wasm comes from.
//
//   import { analyze } from "reactant-analyzer";
//
//   const { report } = await analyze({ files: { "App.tsx": source } });
//
// Analysis holds the thread while it runs, so an interactive host should
// call this from a Worker.
import type { ReactantApi } from "./types.d.ts";

export type * from "./types.d.ts";

export const analyze: ReactantApi["analyze"];
export const run: ReactantApi["run"];
export const rules: ReactantApi["rules"];
export const explain: ReactantApi["explain"];
export const help: ReactantApi["help"];
export const packSpecs: ReactantApi["packSpecs"];
export const validatePack: ReactantApi["validatePack"];
export const hostConstants: ReactantApi["hostConstants"];

/** A caller-side mistake, or a core usage error. `exitCode` is always 2. */
export class UsageError extends Error {
  readonly exitCode: 2;
}

/**
 * Instantiate from an explicit source: a URL, a `Response`, the raw bytes,
 * or a compiled module. Optional — the first `analyze` otherwise fetches the
 * `.wasm` next to the glue. Later calls reuse the instance.
 */
export function initWasm(
  source?: string | URL | Response | BufferSource | WebAssembly.Module,
): Promise<unknown>;
