// argv → { command, explainRule, paths, options, configPath, schemasOut }.
// Thin and enumerable: every flag maps 1:1 onto the wasm envelope; anything
// unknown is a usage error (exit 2), matching the native clap behavior.
"use strict";

const BOOL_FLAGS = {
  "--info": "info",
  "--show-clean": "showClean",
  "--trace": "trace",
  "--verbose": "verbose",
  "--all-roots": "allRoots",
  "--follow-imports": "followImports",
  "--no-color": "noColor",
};

const VALUE_FLAGS = {
  "--format": ["format", ["human", "json"]],
  "--fail-on": ["failOn", ["error", "warning", "never"]],
  "--project": ["project", ["auto", "vite", "next", "plain"]],
};

function parse(argv) {
  const out = {
    command: "check",
    explainRule: null,
    paths: [],
    configPath: null,
    schemasOut: null,
    packsInput: null,
    packsOut: null,
    options: {
      info: false,
      showClean: false,
      trace: false,
      verbose: false,
      allRoots: false,
      followImports: false,
      noColor: false,
      entry: [],
      excludeDir: [],
      format: null,
      failOn: null,
      project: null,
      rule: [],
      ignoreRule: [],
    },
  };

  let args = [...argv];
  if (["check", "rules", "explain", "schemas", "packs"].includes(args[0])) {
    out.command = args.shift();
  }
  if (out.command === "packs") {
    // `packs build <input> [--out <path>]` — the JS→JSON authoring step.
    if (args.shift() !== "build") {
      throw new UsageError("packs: unknown subcommand (expected `packs build <file>`)");
    }
    if (!args.length || args[0].startsWith("--")) {
      throw new UsageError("packs build: missing input file");
    }
    out.packsInput = args.shift();
  }
  if (out.command === "explain") {
    if (!args.length || args[0].startsWith("--")) {
      throw new UsageError("explain: missing rule name");
    }
    out.explainRule = args.shift();
  }

  while (args.length) {
    const a = args.shift();
    if (a in BOOL_FLAGS) {
      out.options[BOOL_FLAGS[a]] = true;
    } else if (a in VALUE_FLAGS) {
      const [key, valid] = VALUE_FLAGS[a];
      const v = args.shift();
      if (!valid.includes(v)) {
        throw new UsageError(`invalid value for ${a}: ${v ?? "(missing)"}`);
      }
      out.options[key] = v;
    } else if (a === "--entry" || a === "--exclude-dir") {
      const v = args.shift();
      if (v == null) throw new UsageError(`${a}: missing value`);
      const into = a === "--entry" ? out.options.entry : out.options.excludeDir;
      into.push(...v.split(",").map((s) => s.trim()));
    } else if (a === "--rule" || a === "--ignore-rule") {
      const v = args.shift();
      if (v == null) throw new UsageError(`${a}: missing value`);
      (a === "--rule" ? out.options.rule : out.options.ignoreRule).push(v);
    } else if (a === "--config") {
      out.configPath = args.shift();
      if (out.configPath == null) throw new UsageError("--config: missing value");
    } else if (a === "--out" && out.command === "schemas") {
      out.schemasOut = args.shift();
      if (out.schemasOut == null) throw new UsageError("--out: missing value");
    } else if (a === "--out" && out.command === "packs") {
      out.packsOut = args.shift();
      if (out.packsOut == null) throw new UsageError("--out: missing value");
    } else if (a === "--help" || a === "-h") {
      out.command = "help";
    } else if (a.startsWith("--")) {
      throw new UsageError(`unknown flag ${a}`);
    } else {
      out.paths.push(a);
    }
  }
  return out;
}

class UsageError extends Error {}

module.exports = { parse, UsageError };
