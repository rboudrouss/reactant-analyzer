// `npx reactant` — argv in, the core's streams out.
//
// The CLI is a *consumer of the programmatic API*, not a parallel path: it
// parses argv, reads the project off disk (lib/project.js, shared with
// `analyzeProject`), and hands the result to the same `run` a script calls.
// An option therefore cannot exist for one host and not the other.

import fs from "node:fs";
import path from "node:path";

import { makeApi } from "./api.js";
import { parse } from "./args.js";
import { loadCore, tryLoadCore } from "./core-node.js";
import { UsageError } from "./envelope.js";
import * as packs from "./packs.js";
import { projectInput } from "./project.js";

const api = makeApi(loadCore);

export async function main(argv) {
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

  if (parsed.command === "help") {
    // Rendered by the core (same bytes as the native `reactant help`), and
    // before any config is read: a broken config must not hide the help.
    process.stdout.write(await api.help({ color: useColor(parsed.options.noColor) }));
    return 0;
  }
  if (parsed.command === "schemas") {
    return schemas(parsed.schemasOut);
  }
  if (parsed.command === "packs") {
    // Authoring-time codegen (ADR-023 §5): evaluate the JS pack, validate
    // through the core, write the JSON that gets committed. A missing or old
    // wasm bundle degrades to write-without-validate (packs.js says so
    // loudly), hence `tryLoadCore`.
    return packs.build(parsed.packsInput, parsed.packsOut, await tryLoadCore());
  }

  const { noColor, ...options } = parsed.options;
  let project;
  try {
    project = await projectInput(api, parsed.paths, {
      configPath: parsed.configPath,
      followImports: options.followImports,
      withFiles: parsed.command === "check",
    });
  } catch (e) {
    if (e instanceof UsageError) {
      process.stderr.write(`[error] ${e.message}\n`);
      return 2;
    }
    throw e;
  }

  // `run`, not `analyze`: a non-zero exit is the CLI's product, and an
  // unknown rule name must print the core's message rather than throw.
  const out = await api.run({
    command: parsed.command,
    explainRule: parsed.explainRule,
    paths: parsed.paths,
    ...project,
    ...options,
    color: useColor(noColor),
  });
  process.stderr.write(out.stderr);
  process.stdout.write(out.stdout);
  return out.exitCode;
}

// Colors are on iff: no `--no-color`, stdout is a terminal, and NO_COLOR is
// absent or empty (https://no-color.org). Mirrors src/cli/color.rs.
function useColor(noColor) {
  return !noColor && Boolean(process.stdout.isTTY) && !(process.env.NO_COLOR ?? "");
}

// The shipped schemas (generated at build time by the native binary from
// the same types the core validates with).
function schemas(outDir) {
  const dir = path.join(import.meta.dirname, "..", "schemas");
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
