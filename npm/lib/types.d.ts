// The API's types, and the calls both entry points expose.
//
// `Report` and friends mirror the JSON reporter's wire schema v2
// (src/driver/json.rs, documented in docs/usage.md), which is the stability
// contract: `analyze` hands the document back verbatim rather than mapping
// it into a second shape that would have to be kept in step.
//
// Hand-written for now. lib/pack.d.ts is generated from the same schemars
// output the validator compiles from, and these types want the same
// treatment once the reporter DTOs carry a `JsonSchema` derive.

/** cwd-relative POSIX path → the file's text. */
export type FileMap = Record<string, string> | Map<string, string>;

/** A pack's JSON, as text or as the object to serialize. */
export interface PackInput {
  /** The spec that named it. Error messages quote this. */
  name?: string;
  json: string | Record<string, unknown>;
}

export interface Options {
  /** Report Info diagnostics: the analyzer's own blind spots. */
  info?: boolean;
  /** List components with no findings. */
  showClean?: boolean;
  /** Render each finding's witness chain. */
  trace?: boolean;
  verbose?: boolean;
  /** Analyze every component, not only the reachable roots. */
  allRoots?: boolean;
  /** Close over resolved import edges beyond the named paths. */
  followImports?: boolean;
  /** Emit ANSI colors. Off by default, since a string consumer wants none. */
  color?: boolean;
  entry?: string[] | string;
  excludeDir?: string[] | string;
  /** Run only these rules. */
  rule?: string[] | string;
  ignoreRule?: string[] | string;
  format?: "human" | "json";
  /** Which severity makes `exitCode` non-zero. Defaults to `"warning"`. */
  failOn?: "error" | "warning" | "never";
  /** Project kind. `"auto"` detects it from the tree's own markers. */
  project?: "auto" | "vite" | "next" | "plain";
}

export interface AnalysisInput {
  /** The tree to analyze. Everything the run may read must be in here. */
  files?: FileMap;
  /** Paths within `files` to report on. Defaults to `["."]`. */
  paths?: string[];
  /** Raw `reactant.config.json` text (JSONC), or the object to serialize. */
  config?: string | Record<string, unknown>;
  packs?: PackInput[];
}

export interface AnalyzeInput extends AnalysisInput, Omit<Options, "format"> {}

export interface RunInput extends AnalysisInput, Options {
  command?: "check" | "rules" | "explain";
  explainRule?: string;
}

/** What the core wrote, for any format: the CLI's own three streams. */
export interface RunOutput {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface AnalyzeResult {
  report: Report;
  /** The process exit code the CLI would have returned under `failOn`. */
  exitCode: number;
  /** The run's chatter about itself: pack warnings, `verbose`/`trace`. */
  stderr: string;
}

export interface TextResult {
  text: string;
  stderr: string;
}

export interface PackVerdict {
  name: string;
  rules: string[];
  warnings: string[];
}

export interface HostConstants {
  prunedDirs: string[];
  sourceExtensions: string[];
  configFileName: string;
}

// ── The report (wire schema v2) ──────────────────────────────────────────────

export interface Report {
  version: 2;
  files_analyzed: number;
  parse_errors: ParseError[];
  diagnostics: Diagnostic[];
  /**
   * What the run knows it did not read. Non-empty means `summary.errors` and
   * `summary.warnings` are a lower bound, not a verdict.
   */
  blind_spots: BlindSpot[];
  /** Present only under `followImports`. */
  followed?: Followed;
  summary: Summary;
}

export interface ParseError {
  file: string;
  message: string;
  /**
   * `false` when the file was dropped rather than recovered: its findings
   * are missing, not absent.
   */
  analyzed: boolean;
}

export type Severity = "error" | "warning" | "info";

export interface Diagnostic {
  rule: string;
  severity: Severity;
  /** Registry display name, `Page@src/a/page.tsx` when names collide. */
  component: string;
  /**
   * The file `line`/`col` point into: the anchor's own file, which for a
   * finding inside a cross-file inlined hook is not `component_file`.
   */
  file: string | null;
  component_file: string | null;
  /** 1-indexed. */
  line: number | null;
  /** 0-indexed. */
  col: number | null;
  hook_label: number | null;
  var: string | null;
  message: string;
  /** The witness chain: why the rule fired. */
  notes: Note[];
}

export type NoteKind =
  | "binding"
  | "resolve"
  | "call"
  | "write"
  | "read"
  | "branch"
  | "handler"
  | "cycle-edge"
  | "widen"
  | "mutate"
  | "capture"
  | "init-once";

/** One typed witness step: `message` is the prose, the rest is its structure. */
export interface Note {
  message: string;
  kind: NoteKind;
  hook_label: number | null;
  file: string | null;
  line: number | null;
  col: number | null;
  var?: string;
  name?: string;
  /** `import:<path>` | `local-fn` | `setter` | `unknown` */
  target?: string;
  callee?: string;
  effect_class?: "setter" | "effectful" | "pure-cheap" | "unknown";
  slot?: number;
  value_class?: "fresh" | "same-as-current" | "unknown";
  what?: string;
  desc?: string;
  event?: string;
  from?: string;
  to?: string;
  iteration?: number;
}

export interface BlindSpot {
  kind: "unresolved-aliases" | "unparsed-files" | "unread-imports";
  count: number;
  detail: string;
}

export interface Followed {
  /** Source files reached through resolved import edges. */
  files: number;
  examples: string[];
  /**
   * Findings computed in those files and not reported, because the caller
   * named other paths. Naming these paths reports them.
   */
  withheld: number;
  withheld_examples: string[];
}

export interface Summary {
  errors: number;
  warnings: number;
  infos: number;
  components_analyzed: number;
  exit_code: number;
}

// ── The calls ────────────────────────────────────────────────────────────────

/**
 * What `reactant-analyzer` exports in either host. The signatures live here
 * so the Node and browser declarations cannot describe different APIs; each
 * entry point re-exports them, and adds what only it can do.
 */
export interface ReactantApi {
  /**
   * Check a tree and get the report as data. `report` is the JSON reporter's
   * document verbatim (wire schema v2).
   *
   * Findings are a result, not a failure. Only a usage error throws
   * `UsageError`: a bad option, an unparseable config, a rejected pack.
   */
  analyze(input?: AnalyzeInput): Promise<AnalyzeResult>;

  /**
   * One core invocation, any format: exactly what `npx reactant` would have
   * written, including ANSI when `color` is set. Use it for the human
   * report; use `analyze` for data.
   */
  run(input?: RunInput): Promise<RunOutput>;

  /** The rules table, rendered. Packs in `input` are listed with natives. */
  rules(input?: RunInput): Promise<TextResult>;

  /** One rule's long-form docs. Throws `UsageError` if unknown. */
  explain(rule: string, input?: RunInput): Promise<TextResult>;

  /** The help page, the same bytes as `reactant help`. */
  help(options?: { color?: boolean }): Promise<string>;

  /** The `packs` specs a config names, per the core's own config parser. */
  packSpecs(configText: string): Promise<string[]>;

  /** Validate a pack against the loader a check run uses. */
  validatePack(pack: string | Record<string, unknown>): Promise<PackVerdict>;

  /** Discovery constants, served by the core so hosts cannot drift from it. */
  hostConstants(): Promise<HostConstants>;
}
