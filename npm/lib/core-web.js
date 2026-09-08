// Loading the core in a browser (or any bundler that resolves the `browser`
// export condition).
//
// Same glue, same `.wasm` as [core-node.js](./core-node.js) — see its header.
// The default path fetches the `.wasm` sitting next to the glue, which is
// what Vite, webpack and a plain `<script type="module">` all resolve
// correctly; `initWasm` overrides it for hosts that serve the bytes from
// somewhere else (a CDN, a bundler asset URL, an already-compiled module).

import init, * as core from "../dist/reactant_wasm.js";

let ready = null;

/** The core, instantiated once per page (or per worker). */
export function loadCore() {
  return (ready ??= init().then(() => core));
}

/**
 * Instantiate from an explicit source: a URL, a `Response`, the raw bytes,
 * or a compiled `WebAssembly.Module`. Optional — calling `analyze` without
 * it fetches the `.wasm` next to the glue. Later calls reuse the instance.
 */
export function initWasm(source) {
  return (ready ??= init(
    source == null ? undefined : { module_or_path: source },
  ).then(() => core));
}
