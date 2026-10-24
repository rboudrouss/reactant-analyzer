// Greedy shrinking of a program AST while a predicate holds.
//
// The edits are generic over the tree, not per node kind: drop an element of
// any array (a component, a helper, a hook, a statement, a dep, a child, a
// prop), null a nullable field (cleanup, key, cond), replace a node by a
// descendant of the same sort, reset an option to its plain value. An edit that
// leaves a dangling name is not prevented: React crashes on it, the predicate
// requires a valid run, and the edit is rejected. The validity check is the
// semantics, not a second, hand-kept model of scoping.

const EXPR = new Set(["num", "str", "bool", "null", "var", "member", "bin", "not", "cond", "fcall", "obj", "arr", "arrow"]);
const STMT = new Set(["write", "helpercall", "call", "if", "refset"]);
const NULLABLE = new Set(["cleanup", "key", "cond", "spread"]);
const PLAIN = { wrap: "plain", layout: false, lazy: false };

const sortOf = (n) => (EXPR.has(n.kind) ? "expr" : STMT.has(n.kind) ? "stmt" : n.kind);

function clone(x) {
  return structuredClone(x);
}

/** Every path (array of keys) to a node or array under `root`. */
function paths(root) {
  const out = [];
  const walk = (x, path) => {
    if (Array.isArray(x)) {
      out.push(path);
      x.forEach((y, i) => walk(y, [...path, i]));
    } else if (x && typeof x === "object") {
      out.push(path);
      for (const [k, y] of Object.entries(x)) walk(y, [...path, k]);
    }
  };
  walk(root, []);
  return out;
}

function get(root, path) {
  return path.reduce((x, k) => x[k], root);
}

function set(root, path, value) {
  const copy = clone(root);
  if (!path.length) return value;
  const parent = get(copy, path.slice(0, -1));
  parent[path[path.length - 1]] = value;
  return copy;
}

function descendants(node, sort) {
  const out = [];
  const walk = (x, top) => {
    if (Array.isArray(x)) return x.forEach((y) => walk(y, false));
    if (!x || typeof x !== "object") return;
    if (!top && x.kind && sortOf(x) === sort) out.push(x);
    for (const y of Object.values(x)) walk(y, false);
  };
  walk(node, true);
  return out;
}

/** Candidate programs one edit smaller than `p`, biggest cuts first. */
export function* candidates(p) {
  const all = paths(p);
  // 1. drop array elements, outermost first (a whole component before a statement).
  for (const path of all) {
    const x = get(p, path);
    const key = path[path.length - 1];
    // Only named lists shrink; a `[key, value]` pair sits at a numeric index.
    if (!Array.isArray(x) || typeof key !== "string" || key === "fields" || key === "args") continue;
    for (let i = x.length - 1; i >= 0; i--) {
      if (key === "comps" && x[i].name === "App") continue;
      const y = x.slice(0, i).concat(x.slice(i + 1));
      yield set(p, path, y);
    }
  }
  for (const path of all) {
    const x = get(p, path);
    if (Array.isArray(x) || !x) continue;
    const key = path[path.length - 1];
    // 2. null a nullable field; turn deps into no deps array only from [].
    if (NULLABLE.has(key)) yield set(p, path, null);
    // 3. splice a compound statement into its branch.
    if (x.kind === "if" && Array.isArray(get(p, path.slice(0, -1)))) {
      const arrPath = path.slice(0, -1);
      const arr = get(p, arrPath);
      const i = path[path.length - 1];
      for (const branch of [x.then, x.else]) yield set(p, arrPath, [...arr.slice(0, i), ...branch, ...arr.slice(i + 1)]);
    }
    // 4. replace by a same-sort descendant, then by a literal.
    if (x.kind && (EXPR.has(x.kind) || STMT.has(x.kind))) {
      for (const d of descendants(x, sortOf(x))) yield set(p, path, clone(d));
      if (EXPR.has(x.kind) && x.kind !== "num") yield set(p, path, { kind: "num", v: 0 });
    }
    // 5. reset options to their plain value.
    if (x.kind) {
      for (const [k, plain] of Object.entries(PLAIN)) {
        if (k in x && x[k] !== plain) yield set(p, path, { ...clone(x), [k]: plain });
      }
      if (x.kind === "write" && x.wrap === "plain" && x.cond) yield set(p, path, { ...clone(x), cond: undefined });
    }
  }
}

/**
 * Shrinks `p` while `keep(candidate)` resolves true. Restarts from the first
 * edit after every success; stops at a fixpoint or after `maxTests` tests.
 */
export async function shrink(p, keep, { maxTests = 600, onStep } = {}) {
  let best = p;
  let tests = 0;
  let improved = true;
  while (improved && tests < maxTests) {
    improved = false;
    for (const c of candidates(best)) {
      if (++tests > maxTests) break;
      if (await keep(c).catch(() => false)) {
        best = c;
        improved = true;
        onStep?.(best, tests);
        break;
      }
    }
  }
  return { program: best, tests };
}
