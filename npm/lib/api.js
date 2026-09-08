// The programmatic API, over an injected core loader.
//
// One implementation for Node and for the browser: the only host-specific
// thing is how the wasm bytes arrive, and that is the `loadCore` argument.
// No `node:` import reaches this file.
//
// Layering: `run` is the CLI's own call — it returns the three streams the
// core returns, for any format. `analyze` is `run` with the JSON reporter,
// parsed. Findings are a result, not an error: only a usage error (exit 2)
// throws, because a caller cannot act on a report that was never produced.

import { UsageError, buildEnvelope, parseOutput } from "./envelope.js";

export function makeApi(loadCore) {
  /**
   * One core invocation. `{ exitCode, stdout, stderr }`, exactly what
   * `npx reactant` would have written — including ANSI when `color` is set.
   */
  async function run(input = {}) {
    const envelope = buildEnvelope(input);
    const core = await loadCore();
    return parseOutput(core.run(JSON.stringify(envelope)));
  }

  /**
   * A check, reported structurally. `report` is the JSON reporter's document
   * verbatim (schema v2, `docs/usage.md`) — the wire schema is the stability
   * contract, so this API adds no second shape to keep in step with it.
   *
   * Returns `{ report, exitCode, stderr }`: findings live in the report, the
   * exit code answers `--fail-on`, and stderr carries what the run wants to
   * say about itself (pack warnings, `verbose`/`trace` chatter).
   */
  async function analyze(input = {}) {
    if (input.format != null && input.format !== "json") {
      throw new UsageError(
        `analyze() always reports JSON; use run({ format: "${input.format}" }) for text`,
      );
    }
    const out = await run({ ...input, command: "check", format: "json" });
    if (!out.stdout) throw usageError(out);
    return { report: JSON.parse(out.stdout), exitCode: out.exitCode, stderr: out.stderr };
  }

  /** The rules table, rendered. `{ text, stderr }`. */
  async function rules(input = {}) {
    return text(await run({ ...input, command: "rules" }));
  }

  /** One rule's long-form documentation, rendered. `{ text, stderr }`. */
  async function explain(rule, input = {}) {
    return text(await run({ ...input, command: "explain", explainRule: rule }));
  }

  /** The help page — the same bytes as `reactant help`. */
  async function help({ color = false } = {}) {
    const core = await loadCore();
    return core.helpPage(Boolean(color));
  }

  /** The `packs` specs a config names, parsed by the core's own parser. */
  async function packSpecs(configText) {
    const core = await loadCore();
    const verdict = JSON.parse(core.packSpecs(configText));
    if (verdict.error) throw new UsageError(verdict.error);
    return verdict.ok;
  }

  /**
   * Validate one pack against the same loader a check run uses.
   * `{ name, rules, warnings }`; throws `UsageError` when the core rejects it.
   */
  async function validatePack(pack) {
    const core = await loadCore();
    const json = typeof pack === "string" ? pack : JSON.stringify(pack);
    const verdict = JSON.parse(core.validatePack(json));
    if (verdict.error) throw new UsageError(verdict.error);
    return verdict.ok;
  }

  /** Discovery constants the core serves so hosts cannot drift from it. */
  async function hostConstants() {
    const core = await loadCore();
    return JSON.parse(core.hostConstants());
  }

  return { run, analyze, rules, explain, help, packSpecs, validatePack, hostConstants, loadCore };
}

/** A text-command result, with the core's usage errors raised. */
function text(out) {
  if (out.exitCode === 2) throw usageError(out);
  return { text: out.stdout, stderr: out.stderr };
}

/** The core's `[error] …` line, as an exception. */
function usageError(out) {
  const message = (out.stderr || out.stdout || "the core produced no output")
    .replace(/^\[error\]\s*/m, "")
    .trim();
  return new UsageError(message);
}
