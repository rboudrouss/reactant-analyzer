// The browser entry point.
//
// Same API as the Node entry (lib/api.js), same `.wasm`, same glue — the
// only difference is that the bytes are fetched instead of read from disk.
// There is no `node:` import in this module's graph, so a bundler pulls in
// nothing beyond the wasm.
//
//   import { analyze } from "reactant-analyzer";
//
//   const { report } = await analyze({ files: { "App.tsx": source } });
//   for (const d of report.diagnostics) console.log(d.rule, d.line, d.message);
//
// Analysis is synchronous once the wasm is up and holds the thread for the
// duration, so an interactive host should run it in a Worker.

import { makeApi } from "./api.js";
import { initWasm, loadCore } from "./core-web.js";
import { UsageError } from "./envelope.js";

const api = makeApi(loadCore);

export const { run, analyze, rules, explain, help, packSpecs, validatePack, hostConstants } = api;
export { UsageError, initWasm };
