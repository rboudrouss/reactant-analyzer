// The Node entry point: the programmatic API, plus the one thing a browser
// cannot offer — reading a project off disk.
//
// `analyze` and friends are the same implementations the browser entry
// exports (lib/api.js); only the wasm loader differs. The CLI is a separate
// entry (lib/cli.js) built on these same calls, so `npx reactant` and a
// script cannot disagree about what an option means.

import { makeApi } from "./api.js";
import { loadCore } from "./core-node.js";
import { UsageError } from "./envelope.js";
import { projectInput } from "./project.js";

const api = makeApi(loadCore);

export const { run, analyze, rules, explain, help, packSpecs, validatePack, hostConstants } = api;
export { UsageError };

/** Inputs `analyzeProject` derives from disk and will not take from a caller. */
const DISCOVERED = ["files", "config", "packs"];

/**
 * Check a real project: walk `paths`, discover `reactant.config.json`
 * (or read `configPath`), resolve the packs it names, then `analyze`.
 * The disk-reading half of `npx reactant check`, minus the reporting.
 *
 * For a virtual tree — an editor buffer, a test fixture, a playground —
 * call `analyze({ files })` directly instead.
 */
export async function analyzeProject(paths = ["."], input = {}) {
  for (const key of DISCOVERED) {
    if (key in input) {
      throw new UsageError(
        `analyzeProject reads \`${key}\` from disk; pass it to analyze() instead`,
      );
    }
  }
  const { configPath = null, ...options } = input;
  const project = await projectInput(api, paths, {
    configPath,
    followImports: Boolean(options.followImports),
  });
  return api.analyze({ ...project, ...options, paths });
}
