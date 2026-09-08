// Loading the core under Node.
//
// The bundle is wasm-bindgen's `--target web` output — the SAME glue and the
// SAME `.wasm` the browser loads. Only the way the bytes arrive differs:
// `readFileSync` + `initSync` here, `fetch` there. That is why the package
// ships one artifact instead of one per host (the `.wasm` is byte-identical
// across wasm-bindgen targets; only the glue differs, and the web glue runs
// in Node too).
//
// The glue is ESM, so it loads through `import()` — hence an async
// `loadCore`. The CLI entry point already resolves a promise.

import { readFileSync } from "node:fs";
import { join } from "node:path";

const GLUE = "../dist/reactant_wasm.js";
const WASM = join(import.meta.dirname, "..", "dist", "reactant_wasm_bg.wasm");

let cached = null;

/**
 * The core, instantiated once per process. Throws a readable error when the
 * bundle is absent (a dev tree that has not run `npm/build.sh`).
 */
export async function loadCore() {
  if (cached) return cached;
  const core = await tryImport();
  if (!core) {
    throw new Error(
      "the wasm bundle is missing (npm/dist/reactant_wasm.js); build it with npm/build.sh",
    );
  }
  cached = core;
  return cached;
}

/** `loadCore` for callers that can degrade: `null` instead of throwing. */
export async function tryLoadCore() {
  if (cached) return cached;
  const core = await tryImport();
  if (core) cached = core;
  return core;
}

async function tryImport() {
  let glue;
  try {
    glue = await import(GLUE);
  } catch (e) {
    if (e && (e.code === "ERR_MODULE_NOT_FOUND" || e.code === "MODULE_NOT_FOUND")) return null;
    throw e;
  }
  glue.initSync({ module: readFileSync(WASM) });
  return glue;
}
