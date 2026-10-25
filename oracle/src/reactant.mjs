// Runs the analyzer on one program and reduces its JSON report to the claims
// the oracle can check (docs/semantics.md §5).

import { execFile } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { splitFiles } from "./files.mjs";

const here = dirname(fileURLToPath(import.meta.url));

export function reactantBinary() {
  return process.env.REACTANT_BIN ?? resolve(here, "../../target/release/reactant");
}

/**
 * The rules whose finding asserts that the program does not settle on its
 * own. `cross-setter-in-render` also accepts React's "Cannot update a
 * component while rendering a different component" as its defect.
 */
export const LOOP_RULES = new Set(["infinite-loop", "cross-component-infinite-loop", "setter-in-render", "cross-setter-in-render"]);

// At most this many analyzer processes at once, whatever the number of
// programs in flight: a shrink of every violation in parallel would otherwise
// start one per candidate.
let slots = 1;
const waiting = [];
export function setConcurrency(n) {
  slots = Math.max(1, n);
}
async function acquire() {
  if (slots > 0) return void slots--;
  await new Promise((res) => waiting.push(res));
}
function release() {
  const next = waiting.shift();
  if (next) next();
  else slots++;
}

/** Analyzes the files of `source` (files.mjs) as one project; returns the parsed report. */
export async function analyze(source, opts) {
  await acquire();
  try {
    return await analyzeNow(source, opts);
  } finally {
    release();
  }
}

function analyzeNow(source, { bin = reactantBinary() } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "reactant-oracle-"));
  for (const f of splitFiles(source)) writeFileSync(join(dir, f.name), f.text);
  return new Promise((res, rej) => {
    execFile(
      bin,
      ["check", "--format", "json", "--fail-on", "never", "--info", "--project", "plain", dir],
      { cwd: dir, maxBuffer: 64 << 20, timeout: 60_000 },
      (err, stdout, stderr) => {
        rmSync(dir, { recursive: true, force: true });
        if (err) return rej(new Error(`reactant failed: ${err.message}\n${stderr}`));
        try {
          res(JSON.parse(stdout));
        } catch (e) {
          rej(new Error(`reactant output is not JSON: ${e.message}\n${stdout.slice(0, 500)}`));
        }
      },
    );
  });
}

/** The checkable part of a report. */
export function claims(report) {
  const loops = [];
  const limits = [];
  for (const d of report.diagnostics) {
    if (LOOP_RULES.has(d.rule) && (d.severity === "error" || d.severity === "warning")) {
      loops.push({ rule: d.rule, severity: d.severity, component: d.component, line: d.line, message: d.message });
    } else if (d.rule === "analysis-limit") {
      limits.push({ component: d.component, message: d.message });
    }
  }
  return {
    loops,
    limits,
    blindSpots: report.blind_spots?.length ?? 0,
    parseErrors: report.parse_errors?.length ?? 0,
    components: report.summary?.components_analyzed ?? 0,
  };
}
