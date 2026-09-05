// `reactant packs build` — the JS/TS→JSON authoring path (ADR-023 §5).
//
// A pack may be AUTHORED as a JS module (the eslint.config.js model): types,
// tests, shared constants, generate-N-rules-from-a-table. The module is
// evaluated HERE, at authoring time, and the resulting JSON is what gets
// committed — the analyzer (native or wasm) only ever consumes the inert
// JSON, so running a pack never executes author code and nothing forks
// between the two hosts.
//
// Validation is the core's: when the wasm bundle exposes `validatePack`, the
// generated JSON goes through the exact `load_pack` a check run uses before
// being written. Without it (stale local dist), the file is written with a
// note — the core re-validates everything it receives on the next check
// anyway (ADR-022 §6: the host is never a trust boundary).
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

/** Default output path: `team.pack.js` → `team.pack.json`, `team.js` → `team.json`. */
function defaultOut(input) {
  const dir = path.dirname(input);
  const base = path.basename(input).replace(/\.(js|mjs|cjs|ts|mts|cts)$/, "");
  return path.join(dir, `${base}.json`);
}

/** Evaluate the authored module to a plain pack object. */
async function evaluate(input) {
  const abs = path.resolve(input);
  let mod;
  try {
    // import() loads both ESM and CJS; on Node builds with type stripping it
    // also loads plain .ts. CJS callers keep working — no loader config.
    mod = await import(pathToFileURL(abs).href);
  } catch (e) {
    if (e && e.code === "ERR_UNKNOWN_FILE_EXTENSION") {
      throw new Error(
        `${input}: this Node cannot load that extension directly. ` +
          `compile it to .js first (or use a Node with type stripping)`,
      );
    }
    throw new Error(`${input}: evaluation failed: ${e.message}`);
  }
  let pack = mod && "default" in mod ? mod.default : mod;
  // ESM/CJS interop can double-wrap the default export.
  if (pack && typeof pack === "object" && "default" in pack && !("rules" in pack)) {
    pack = pack.default;
  }
  if (typeof pack === "function") pack = await pack();
  if (pack == null || typeof pack !== "object" || Array.isArray(pack)) {
    throw new Error(
      `${input}: the module must export a pack object ` +
        `({ schemaVersion, name, rules }), got ${Array.isArray(pack) ? "an array" : typeof pack}`,
    );
  }
  return pack;
}

/**
 * Build one pack: evaluate `input`, validate through the wasm core when
 * available, write pretty JSON to `out` (committed artifact — diffable).
 * Returns a process exit code.
 */
async function build(input, outPath, wasm, io = { out: process.stdout, err: process.stderr }) {
  let pack;
  try {
    pack = await evaluate(input);
  } catch (e) {
    io.err.write(`[error] ${e.message}\n`);
    return 2;
  }

  const json = JSON.stringify(pack, null, 2) + "\n";

  if (wasm && typeof wasm.validatePack === "function") {
    const verdict = JSON.parse(wasm.validatePack(json));
    if (verdict.error) {
      io.err.write(`[error] ${input}: ${verdict.error}\n`);
      return 2;
    }
    for (const w of verdict.ok.warnings) {
      io.err.write(`[warning] ${w}\n`);
    }
    const target = outPath ?? defaultOut(input);
    fs.mkdirSync(path.dirname(path.resolve(target)), { recursive: true });
    fs.writeFileSync(target, json);
    io.out.write(
      `wrote ${target}: pack \`${verdict.ok.name}\`, ${verdict.ok.rules.length} rule(s): ` +
        `${verdict.ok.rules.join(", ")}\n`,
    );
    return 0;
  }

  // No validator in this bundle: still write (the core re-validates on the
  // next check), but say so — silence would read as "validated".
  const target = outPath ?? defaultOut(input);
  fs.mkdirSync(path.dirname(path.resolve(target)), { recursive: true });
  fs.writeFileSync(target, json);
  io.out.write(`wrote ${target} (not validated, this bundle lacks validatePack; ` +
    `the core validates it on the next check)\n`);
  return 0;
}

module.exports = { build, defaultOut, evaluate };
