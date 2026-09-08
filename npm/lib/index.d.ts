// The Node entry point.
//
//   import { analyzeProject } from "reactant-analyzer";
//
//   const { report, exitCode } = await analyzeProject(["src"]);
//   for (const d of report.diagnostics) {
//     console.log(`${d.file}:${d.line} ${d.rule} ${d.message}`);
//   }
import type { AnalyzeInput, AnalyzeResult, ReactantApi } from "./types.d.ts";

export type * from "./types.d.ts";

export const analyze: ReactantApi["analyze"];
export const run: ReactantApi["run"];
export const rules: ReactantApi["rules"];
export const explain: ReactantApi["explain"];
export const help: ReactantApi["help"];
export const packSpecs: ReactantApi["packSpecs"];
export const validatePack: ReactantApi["validatePack"];
export const hostConstants: ReactantApi["hostConstants"];

/** A caller-side mistake, or a core usage error. `exitCode` is always 2. */
export class UsageError extends Error {
  readonly exitCode: 2;
}

/** Options `analyzeProject` derives from disk instead of taking. */
export interface ProjectInput
  extends Omit<AnalyzeInput, "files" | "config" | "packs"> {
  /** An explicit config file, as `--config` does. Discovered otherwise. */
  configPath?: string;
}

/**
 * Check a real project: walk `paths`, discover `reactant.config.json`,
 * resolve the packs it names, then `analyze`. The disk-reading half of
 * `npx reactant check`, minus the reporting.
 *
 * For a virtual tree — an editor buffer, a test fixture, a playground —
 * call `analyze({ files })` instead.
 */
export function analyzeProject(
  paths?: string[],
  input?: ProjectInput,
): Promise<AnalyzeResult>;
