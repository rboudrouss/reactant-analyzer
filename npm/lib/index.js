// npx reactant — thin composition over the wasm core.
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const { parse, UsageError } = require("./args.js");
const host = require("./host.js");

const HELP = `reactant — static analyzer for React hook bugs (WASM build)

Usage:
  reactant [check] [paths…] [--info] [--show-clean] [--trace] [--verbose]
           [--all-roots] [--entry <names>] [--format human|json]
           [--fail-on error|warning|never] [--project auto|vite|next|plain]
           [--rule <name>]… [--ignore-rule <name>]… [--no-color]
           [--exclude-dir <names>] [--config <path>]
  reactant rules [--config <path>]
  reactant explain <rule> [--config <path>]
  reactant schemas [--out <dir>]
  reactant packs build <pack.js> [--out <pack.json>]
`;

function main(argv) {
  let parsed;
  try {
    parsed = parse(argv);
  } catch (e) {
    if (e instanceof UsageError) {
      process.stderr.write(`[error] ${e.message}\n`);
      return 2;
    }
    throw e;
  }
  if (parsed.command === "help" || (parsed.command === "check" && !argv.length)) {
    process.stdout.write(HELP);
    return parsed.command === "help" ? 0 : 2;
  }
  if (parsed.command === "schemas") {
    return schemas(parsed.schemasOut);
  }
  if (parsed.command === "packs") {
    // Authoring-time codegen (ADR-023 §5): evaluate the JS pack, validate
    // through the core, write the JSON that gets committed. Async (dynamic
    // import); the bin resolves the returned promise. A missing/old wasm
    // bundle degrades to write-without-validate (packs.js says so loudly).
    let wasm = null;
    try {
      wasm = require("../dist/reactant_wasm.js");
    } catch {
      // dev tree without a built dist — packs.js handles null.
    }
    return require("./packs.js").build(parsed.packsInput, parsed.packsOut, wasm);
  }

  const wasm = require("../dist/reactant_wasm.js");
  const constants = JSON.parse(wasm.hostConstants());

  const root = parsed.command === "check" ? host.projectRoot(parsed.paths) : ".";
  let configText, configDir;
  try {
    ({ text: configText, dir: configDir } = host.readConfigText(
      parsed.configPath,
      root,
      constants.configFileName,
    ));
  } catch (e) {
    process.stderr.write(`[error] cannot read ${parsed.configPath}: ${e.message}\n`);
    return 2;
  }

  // Pack specs come from the core's own config parser (never JSONC in JS).
  let packs = [];
  if (configText != null) {
    const specs = JSON.parse(wasm.packSpecs(configText));
    if (specs.error) {
      process.stderr.write(`[error] ${specs.error}\n`);
      return 2;
    }
    try {
      packs = host.resolvePacks(specs.ok, configDir);
    } catch (e) {
      process.stderr.write(`[error] ${e.message}\n`);
      return 2;
    }
  }

  const color =
    !parsed.options.noColor &&
    process.stdout.isTTY &&
    !(process.env.NO_COLOR ?? "");

  const input = {
    command: parsed.command,
    explainRule: parsed.explainRule,
    paths: parsed.paths,
    files:
      parsed.command === "check" ? host.buildFileMap(parsed.paths, constants) : {},
    config: configText,
    packs,
    options: {
      info: parsed.options.info,
      showClean: parsed.options.showClean,
      trace: parsed.options.trace,
      verbose: parsed.options.verbose,
      allRoots: parsed.options.allRoots,
      entry: parsed.options.entry,
      format: parsed.options.format,
      failOn: parsed.options.failOn,
      project: parsed.options.project,
      rule: parsed.options.rule,
      ignoreRule: parsed.options.ignoreRule,
      color: Boolean(color),
    },
  };

  const out = JSON.parse(wasm.run(JSON.stringify(input)));
  process.stderr.write(out.stderr);
  process.stdout.write(out.stdout);
  return out.exitCode;
}

// The shipped schemas (generated at build time by the native binary from
// the same types the core validates with).
function schemas(outDir) {
  const dir = path.join(__dirname, "..", "schemas");
  const names = ["pack.schema.json", "reactant-config.schema.json"];
  if (outDir) {
    fs.mkdirSync(outDir, { recursive: true });
    for (const n of names) {
      fs.copyFileSync(path.join(dir, n), path.join(outDir, n));
      process.stdout.write(`wrote ${path.join(outDir, n)}\n`);
    }
  } else {
    const doc = {};
    for (const n of names) {
      doc[n] = JSON.parse(fs.readFileSync(path.join(dir, n), "utf8"));
    }
    process.stdout.write(JSON.stringify(doc, null, 2) + "\n");
  }
  return 0;
}

module.exports = { main };
