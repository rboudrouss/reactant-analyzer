// Everything the envelope needs that only a filesystem can answer: the
// config text, the resolved packs, the file map.
//
// Both fs-backed callers come through here — the CLI and `analyzeProject` —
// so "what a project run reads" is decided once. The browser never imports
// this file.

import { UsageError } from "./envelope.js";
import * as host from "./host.js";

/**
 * Read a project off disk into envelope inputs.
 *
 * `withFiles` is false for the commands that do not analyze (`rules`,
 * `explain`): they still need the config and its packs — a custom rule is
 * only explainable once loaded — but no file map, and their root is the cwd
 * rather than the first path argument.
 */
export async function projectInput(
  api,
  paths,
  { configPath = null, followImports = false, withFiles = true } = {},
) {
  const constants = await api.hostConstants();
  const root = withFiles ? host.projectRoot(paths) : ".";

  let text, dir;
  try {
    ({ text, dir } = host.readConfigText(configPath, root, constants.configFileName));
  } catch (e) {
    throw new UsageError(`cannot read ${configPath}: ${e.message}`);
  }

  // Specs come from the core's own config parser — never JSONC in JS.
  let packs = [];
  if (text != null) {
    const specs = await api.packSpecs(text);
    try {
      packs = host.resolvePacks(specs, dir);
    } catch (e) {
      throw new UsageError(e.message);
    }
  }

  return {
    files: withFiles ? host.buildFileMap(paths, constants, followImports) : {},
    config: text,
    packs,
  };
}
