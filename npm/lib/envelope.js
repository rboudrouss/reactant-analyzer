// The wasm input envelope, built and validated in one place.
//
// This module is the whole reason the CLI and the programmatic API cannot
// drift: both of them come through `buildEnvelope`, so an option exists for
// both hosts or for neither. It is deliberately free of `node:` imports —
// the browser entry point loads it unchanged.
//
// Validation here is ergonomic only (a typo'd option name should say so
// instead of reaching the core as a serde error). It is NOT a trust
// boundary: the core re-parses and re-validates the config, the packs and
// every option it receives (ADR-022 §6).

/** Every `Options` field of the wasm envelope, by kind. */
const BOOL_OPTIONS = [
  "info",
  "showClean",
  "trace",
  "verbose",
  "allRoots",
  "followImports",
  "color",
];
const LIST_OPTIONS = ["entry", "excludeDir", "rule", "ignoreRule"];
const ENUM_OPTIONS = {
  format: ["human", "json"],
  failOn: ["error", "warning", "never"],
  project: ["auto", "vite", "next", "plain"],
};

/** Envelope fields that are not options — the analysis input itself. */
const INPUT_KEYS = ["command", "explainRule", "paths", "files", "config", "packs"];

const COMMANDS = ["check", "rules", "explain"];

/**
 * A caller-side mistake, or a core-side usage error (exit code 2): a bad
 * option, an unparseable config, a rejected pack. Findings never throw.
 */
export class UsageError extends Error {
  constructor(message) {
    super(message);
    this.name = "UsageError";
    this.exitCode = 2;
  }
}

/** `["a", "b"]` or `"a,b"` → `["a", "b"]`, matching the CLI's comma split. */
function toList(value, key) {
  if (value == null) return [];
  const items = Array.isArray(value) ? value : String(value).split(",");
  return items.map((item) => {
    if (typeof item !== "string" && typeof item !== "number") {
      throw new UsageError(`${key}: expected strings, got ${typeof item}`);
    }
    return String(item).trim();
  });
}

/**
 * A flat option bag → the envelope's `options`, with every field present so
 * the wire shape does not depend on what the caller happened to pass.
 */
export function normalizeOptions(options = {}) {
  const known = new Set([...BOOL_OPTIONS, ...LIST_OPTIONS, ...Object.keys(ENUM_OPTIONS)]);
  for (const key of Object.keys(options)) {
    if (!known.has(key)) {
      throw new UsageError(
        `unknown option \`${key}\` (known: ${[...known].sort().join(", ")})`,
      );
    }
  }
  const out = {};
  for (const key of BOOL_OPTIONS) out[key] = Boolean(options[key]);
  for (const key of LIST_OPTIONS) out[key] = toList(options[key], key);
  for (const [key, valid] of Object.entries(ENUM_OPTIONS)) {
    const value = options[key] ?? null;
    if (value !== null && !valid.includes(value)) {
      throw new UsageError(`invalid ${key} \`${value}\` (expected ${valid.join(" | ")})`);
    }
    out[key] = value;
  }
  return out;
}

/** A file map (object or Map) → the envelope's `files`. */
function normalizeFiles(files) {
  if (files == null) return {};
  const entries = files instanceof Map ? [...files] : Object.entries(files);
  const out = {};
  for (const [key, value] of entries) {
    if (typeof value !== "string") {
      throw new UsageError(`files["${key}"]: expected the file text, got ${typeof value}`);
    }
    // Keys reach the core verbatim: it normalizes `.`/`..` itself, and
    // mangling separators here would rename files whose name legitimately
    // holds a backslash. Callers pass cwd-relative POSIX paths.
    out[String(key)] = value;
  }
  return out;
}

/** Raw config text, or an object to serialize — the core is the only parser. */
function normalizeConfig(config) {
  if (config == null) return null;
  if (typeof config === "string") return config;
  if (typeof config === "object") return JSON.stringify(config);
  throw new UsageError(`config: expected text or an object, got ${typeof config}`);
}

/** `[{ name, json }]`, where `json` may be an object to serialize. */
function normalizePacks(packs) {
  if (packs == null) return [];
  if (!Array.isArray(packs)) throw new UsageError("packs: expected an array");
  return packs.map((pack, i) => {
    if (pack == null || typeof pack !== "object") {
      throw new UsageError(`packs[${i}]: expected { name, json }`);
    }
    const json = pack.json;
    if (json == null) throw new UsageError(`packs[${i}]: missing \`json\``);
    return {
      name: String(pack.name ?? `packs[${i}]`),
      json: typeof json === "string" ? json : JSON.stringify(json),
    };
  });
}

/**
 * A flat input — analysis input and options in one object — → the envelope
 * the core deserializes. Unknown keys are a usage error rather than a serde
 * failure inside the wasm (`Input` and `Options` both deny unknown fields).
 */
export function buildEnvelope(input = {}) {
  const analysis = {};
  const options = {};
  for (const [key, value] of Object.entries(input)) {
    if (INPUT_KEYS.includes(key)) analysis[key] = value;
    else options[key] = value;
  }

  const command = analysis.command ?? "check";
  if (!COMMANDS.includes(command)) {
    throw new UsageError(`unknown command \`${command}\` (expected ${COMMANDS.join(" | ")})`);
  }
  const explainRule = analysis.explainRule ?? null;
  if (command === "explain" && !explainRule) {
    throw new UsageError("explain: missing rule name");
  }
  // Not `toList`: a path may legitimately contain a comma.
  const paths =
    analysis.paths == null
      ? []
      : (Array.isArray(analysis.paths) ? analysis.paths : [analysis.paths]).map(String);

  return {
    command,
    explainRule,
    paths,
    files: normalizeFiles(analysis.files),
    config: normalizeConfig(analysis.config),
    packs: normalizePacks(analysis.packs),
    options: normalizeOptions(options),
  };
}

/** The core's output envelope: `{ exitCode, stdout, stderr }`. */
export function parseOutput(text) {
  return JSON.parse(text);
}
