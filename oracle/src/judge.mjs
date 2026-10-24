// The soundness relation between what React did and what the analyzer said
// (docs/semantics.md §5).
//
//   P1  React loops            ⇒ some loop claim at Warning or Error.
//   P2  an Error loop claim on a component React rendered ⇒ React loops
//       (or, for `cross-setter-in-render`, React warned about the cross-render
//       update the rule also names).
//
// P1 broken is a false negative, the thing CLAUDE.md forbids. P2 broken is an
// Error that is not certain, the other thing it forbids. A Warning on a program
// that settles is a tolerated false positive and is only counted.

export const VERDICTS = ["fn", "error-unsound", "parse-error", "fp", "tp", "tn", "invalid"];
/** The classes that break a stated invariant. */
export const VIOLATIONS = new Set(["fn", "error-unsound", "parse-error"]);

export function judge(react, c) {
  if (react.verdict !== "loop" && react.verdict !== "converges") return { verdict: "invalid", why: `react: ${react.verdict} (${react.via})` };
  if (c.parseErrors > 0) return { verdict: "parse-error", why: "reactant could not parse a program React runs" };

  if (react.verdict === "loop") {
    if (c.loops.length) return { verdict: "tp", why: c.loops.map((l) => `${l.severity} ${l.rule} on ${l.component}`).join("; ") };
    const disclosed = c.limits.length > 0 || c.blindSpots > 0;
    return {
      verdict: "fn",
      disclosed,
      why: `React loops (${react.via}); no loop claim${disclosed ? " (an analysis-limit was reported)" : ""}${c.components === 0 ? "; no component detected" : ""}`,
    };
  }

  const unsound = c.loops.filter(
    (l) =>
      l.severity === "error" &&
      (react.renders[l.component] ?? 0) > 0 &&
      !(l.rule === "cross-setter-in-render" && react.warnings.includes("cross-render-update")),
  );
  if (unsound.length) {
    return { verdict: "error-unsound", why: `React settles; ${unsound.map((l) => `error ${l.rule} on ${l.component} (line ${l.line})`).join("; ")}` };
  }
  if (c.loops.length) return { verdict: "fp", why: c.loops.map((l) => `${l.severity} ${l.rule} on ${l.component}`).join("; ") };
  return { verdict: "tn", why: "" };
}
