// Fixture for `reactant packs build` (ADR-023 §5): a pack AUTHORED in JS —
// shared constants and a generate-rules-from-a-table loop, i.e. exactly the
// composition-at-authoring-time the JSON cannot express. `packs build`
// compiles it to team.pack.json; the JSON is what a repo would commit.

/** @type {import("../../lib/pack.d.ts").Pack} */

// One banned-hook rule per entry — the table is the source of truth.
const BANNED = [
  { hook: "useLayoutEffect", wrapper: "useSafeLayoutEffect" },
  { hook: "useInsertionEffect", wrapper: "useStyleEffect" },
];

const bannedRules = BANNED.map(({ hook, wrapper }) => ({
  id: `no-direct-${hook.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`).slice(4)}`,
  docs: {
    description: `${hook} called directly instead of ${wrapper}`,
    why: `${hook} warns during SSR; ${wrapper} swaps it for useEffect on the server`,
    fix: `import ${wrapper} and call it instead`,
  },
  severity: "warning",
  anchor: { relation: "hook_calls", kind: "effect" },
  guards: [{ kind: "origin", of: "anchor", hook: [hook], direct: true }],
  message: `${hook} is called directly — use ${wrapper}`,
}));

module.exports = {
  schemaVersion: 1,
  name: "team",
  rules: [
    ...bannedRules,
    {
      id: "fresh-store-selector",
      docs: {
        description: "store selector returns a fresh reference",
        why: "a fresh reference defeats Object.is — infinite re-render under zustand v5",
        fix: "select primitives, or memoize the selector with useShallow",
      },
      severity: "warning",
      anchor: { relation: "hook_calls", kind: "custom" },
      forEach: { edge: "args", as: "sel" },
      guards: [
        { kind: "name", of: "anchor", one_of: ["useStore"] },
        { kind: "returns", of: "sel", is: ["fresh-reference"] },
      ],
      message: "the selector passed to {anchor.name} returns {sel.returns}",
    },
  ],
};
