// GENERATED from schemas/pack.schema.json by scripts/gen-pack-dts.js — do not edit.
// The schema and the validator compile from the same Rust types, so these
// TypeScript types cannot drift from what the core accepts.
//
// Author a pack as a JS module and compile it with `reactant packs build`:
//
//   /** @type {import("reactant-analyzer/lib/pack").Pack} *​/
//   module.exports = { schemaVersion: 1, name: "team", rules: [ /* … */ ] };
//
// The generated JSON is the committed artifact; the analyzer only ever
// consumes the inert JSON.


export interface PackFile {
  /**
   * Editor-facing schema URL; not interpreted. Accepted for the same reason
   * `reactant.config.json` accepts it — a published schema is only useful if
   * the file is allowed to point at it.
   */
  "$schema"?: string | null;
  /**
   * Pack name: the namespace of every rule id (`<name>/<rule>`).
   */
  "name": string;
  "rules": RuleDef[];
  /**
   * Format version; only `1` exists.
   */
  "schemaVersion": number;
}

/**
 * The anchor: a relation the engine has already resolved (ADR-022 §1) —
 * never a syntax pattern.
 */
export type Anchor = {
  "kind"?: HookKindFilter | null;
  "relation": "hook_calls";
} | {
  "relation": "render_setter_calls";
} | {
  "relation": "hook_origins";
} | {
  "relation": "context_providers";
} | {
  "relation": "jsx_props";
};

/**
 * Total mirror of `CleanupVerdict` (#100): what an effect body returns, seen
 * as teardown. `absent` is the claim — every exit returns nothing at all —
 * and `unknown` folds to the may side (there may be a cleanup), so it is
 * matchable but never actionable as an absence.
 */
export type CleanupName = "present" | "absent" | "unknown";

export type EdgeName = "deps" | "body_setter_calls" | "args" | "writers";

/**
 * What happens to a finding whose must-guard did not certify: `keep` (the
 * default — it survives as a Warning-ceiling finding, ADR-022 §3's free
 * stratification) or `drop` (explicit opt-in for qualification-style rules).
 */
export type ElseBehavior = "keep" | "drop";

/**
 * Typed navigation from the anchor (ADR-022 §2): at most one edge, one
 * binding — no joins.
 */
export interface ForEach {
  "as": string;
  "edge": EdgeName;
}

/**
 * A guard: a predicate over an engine verdict. `must_*` guards certify
 * (attach the `Certified` proof on `All`); the others filter. The `must_`
 * prefix makes polarity visible in the JSON — the §3 load-time warning is
 * a prefix scan.
 */
export type Guard = {
  "is"?: PVal_Array_of_StabilityName | null;
  "kind": "stability";
  "not"?: PVal_Array_of_StabilityName | null;
  "of": string;
} | {
  "is"?: PVal_Array_of_ReturnsName | null;
  "kind": "returns";
  "not"?: PVal_Array_of_ReturnsName | null;
  "of": string;
} | {
  "direct"?: PVal_boolean | null;
  "hook"?: PVal_Array_of_string | null;
  "kind": "origin";
  "of": string;
} | {
  "kind": "in_deps";
  "negate"?: boolean;
  "of": string;
} | {
  "kind": "name";
  "of": string;
  "one_of"?: PVal_Array_of_string | null;
  "prefix"?: PVal_string | null;
} | {
  "kind": "source";
  "of": string;
  "one_of"?: PVal_Array_of_string | null;
  "prefix"?: PVal_string | null;
} | {
  "is"?: PVal_Array_of_IdentityName | null;
  "kind": "identity";
  "not"?: PVal_Array_of_IdentityName | null;
  "of": string;
} | {
  "is"?: PVal_Array_of_CleanupName | null;
  "kind": "cleanup";
  "not"?: PVal_Array_of_CleanupName | null;
  "of": string;
} | {
  "direct"?: PVal_boolean | null;
  "kind": "provenance";
  "of": string;
  "through"?: PVal_Array_of_string | null;
} | {
  "includes": PVal_Array_of_PhaseName;
  "kind": "writer_phases";
  "of": string;
} | {
  "is": PVal_Array_of_UpdaterName;
  "kind": "updater";
  "of": string;
} | {
  "is": PVal_Array_of_ImpureName;
  "kind": "updater_body";
  "of": string;
} | {
  "kind": "same_tick";
  "of": string;
} | {
  /**
   * Name the element binds under inside `guards`. It is the same slot a
   * rule-level `forEach` binding uses, which the quantifier owns for
   * its own subtree — so the outer binding is not visible inside, and
   * this name is not visible in the message.
   */
  "as": string;
  "guards": Guard[];
  "kind": "every";
  "of": string;
} | {
  "equals"?: PVal_uint64 | null;
  "kind": "count";
  "less_than"?: PVal_uint64 | null;
  "more_than"?: PVal_uint64 | null;
  "of": string;
} | {
  "eq": PVal_boolean;
  "kind": "deps_declared";
  "of": string;
} | {
  "else"?: ElseBehavior;
  "kind": "must_setter_on_all_paths";
  "of": string;
} | {
  "else"?: ElseBehavior;
  "kind": "must_dominates_all_exits";
  "of": string;
} | {
  "else"?: ElseBehavior;
  "kind": "must_init_calls_setter";
  "of": string;
} | {
  "else"?: ElseBehavior;
  "kind": "must_hook_is_conditional";
  "of": string;
} | {
  "else"?: ElseBehavior;
  "kind": "must_direct_write";
  "of": string;
} | {
  "guards": Guard[];
  "kind": "any_of";
};

export type HookKindFilter = "state" | "effect" | "memo" | "callback" | "ref" | "custom" | "handler";

/**
 * Total mirror of `ValueIdentity` (#71): what a provider's `value` hands
 * consumers across renders. Two-valued on purpose — `fresh-every-render` is
 * a proven fact, everything else is `unknown` (may side, never actionable).
 */
export type IdentityName = "fresh-every-render" | "unknown";

/**
 * Total mirror of the updater-body purity classifier (ADR-028 §2);
 * ⊤ = `unknown`.
 */
export type ImpureName = "impure" | "unknown";

export type PVal_Array_of_CleanupName = CleanupName[] | {
  "$param": string;
};

export type PVal_Array_of_IdentityName = IdentityName[] | {
  "$param": string;
};

export type PVal_Array_of_ImpureName = ImpureName[] | {
  "$param": string;
};

export type PVal_Array_of_PhaseName = PhaseName[] | {
  "$param": string;
};

export type PVal_Array_of_ReturnsName = ReturnsName[] | {
  "$param": string;
};

export type PVal_Array_of_StabilityName = StabilityName[] | {
  "$param": string;
};

export type PVal_Array_of_UpdaterName = UpdaterName[] | {
  "$param": string;
};

export type PVal_Array_of_string = string[] | {
  "$param": string;
};

export type PVal_boolean = boolean | {
  "$param": string;
};

export type PVal_string = string | {
  "$param": string;
};

export type PVal_uint64 = number | {
  "$param": string;
};

export interface ParamDecl {
  "default": unknown;
  "type": ParamType;
}

export type ParamType = "number" | "string" | "boolean" | "string[]";

/**
 * Total mirror of `WriterPhase` (ADR-027 §1); ⊤ = `unknown`.
 */
export type PhaseName = "render" | "effect" | "memo" | "callback" | "handler" | "unknown" | "deferred" | "cleanup";

/**
 * Total mirror of `ReturnsVerdict`.
 */
export type ReturnsName = "stable" | "fresh-reference" | "unknown";

export interface RuleDef {
  "anchor": Anchor;
  "docs": RuleDocs;
  "forEach"?: ForEach | null;
  "guards"?: Guard[];
  /**
   * Bare rule id (no `/`); addressed as `<pack>/<id>`.
   */
  "id": string;
  /**
   * Message template interpolating navigated entities (`{setter.slot}`)
   * and params (`{param.maxDeps}`); `{{`/`}}` escape braces.
   */
  "message": string;
  /**
   * Declared parameters, referenced as `{"$param": "<name>"}` in leaf
   * constant positions (ADR-022 §4).
   */
  "params"?: { [key: string]: ParamDecl };
  /**
   * Desired severity ceiling (a pin, ADR-022 §3): the effective severity
   * of each finding is `pin ⊓ polarity`, evaluated at emission.
   */
  "severity": SeverityPin;
}

/**
 * Mandatory docs (ADR-022 §5): a custom rule without an explanation is
 * exactly the diagnostic a team learns to ignore.
 */
export interface RuleDocs {
  /**
   * One line — what the rule detects (`reactant rules`).
   */
  "description": string;
  /**
   * Optional minimal buggy snippet.
   */
  "example"?: string | null;
  /**
   * How to fix it.
   */
  "fix": string;
  /**
   * Why it matters (`reactant explain`).
   */
  "why": string;
}

export type SeverityPin = "error" | "warning" | "info";

/**
 * Total mirror of `StabilityVerdict`.
 */
export type StabilityName = "stable" | "versioned" | "per-render" | "unknown";

/**
 * Total mirror of the `writers` updater column (ADR-028 §2); ⊤ = `unknown`.
 */
export type UpdaterName = "functional" | "unknown";

/** The type an authored pack module exports (or returns from a function). */
export type Pack = PackFile;
