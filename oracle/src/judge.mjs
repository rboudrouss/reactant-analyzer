// The soundness relation between what React did and what the analyzer said
// (docs/semantics.md §5).
//
//   P1  React loops            ⇒ a loop claim at Warning or Error on a
//       component that takes part in the loop (rendered in its last renders).
//   P2  an Error loop claim on a component React rendered ⇒ React does not
//       settle (or, for `cross-setter-in-render`, React warned about the
//       cross-render update the rule also names).
//
// P1 broken is a false negative, the thing CLAUDE.md forbids. P2 broken is an
// Error that is not certain, the other thing it forbids. A Warning on a program
// that settles is a tolerated false positive and is only counted. A perpetual
// program (timers re-armed for ever) is neither: the rules do not claim it, and
// it does not settle, so it refutes no Error either.

export const VERDICTS = ["fn", "error-unsound", "parse-error", "fp", "tp", "tn", "perpetual", "invalid"];
/** The classes that break a stated invariant. */
export const VIOLATIONS = new Set(["fn", "error-unsound", "parse-error"]);

const claimText = (ls) => ls.map((l) => `${l.severity} ${l.rule} on ${l.component}`).join("; ");

export function judge(react, c) {
  if (!["loop", "converges", "perpetual"].includes(react.verdict)) return { verdict: "invalid", why: `react: ${react.verdict} (${react.via})` };
  if (c.parseErrors > 0) return { verdict: "parse-error", why: "reactant could not parse a program React runs" };

  if (react.verdict === "perpetual") return { verdict: "perpetual", why: c.loops.length ? claimText(c.loops) : "" };

  if (react.verdict === "loop") {
    const inLoop = new Set(react.inLoop);
    const located = c.loops.filter((l) => inLoop.has(l.component));
    if (located.length) return { verdict: "tp", why: claimText(located) };
    const disclosed = c.limits.length > 0 || c.blindSpots > 0;
    const elsewhere = c.loops.length ? `; claims only outside the loop (${claimText(c.loops)})` : "";
    return {
      verdict: "fn",
      disclosed,
      why: `React loops (${react.via}) through ${react.inLoop.join(", ")}; no loop claim on them${elsewhere}${disclosed ? " (an analysis-limit was reported)" : ""}${c.components === 0 ? "; no component detected" : ""}`,
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
  if (c.loops.length) return { verdict: "fp", why: claimText(c.loops) };
  return { verdict: "tn", why: "" };
}
