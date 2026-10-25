// Random generator of small, well-typed React programs (docs/semantics.md §4).
//
// A program is a list of module helpers and a list of components; the last one
// is `App`, the root, rendered without props. A component may only render the
// components declared before it, so no program recurses. Every name a
// generated expression reads is in scope and of the right type, so a program
// that crashes in React is a generator bug or a shrinker step, never the norm.
//
// The vocabulary is chosen from the shapes the analyzer has been wrong on
// (issues #168-#181): functional updaters, setter calls in `&&` / ternary
// position, `useReducer`, cleanup writes, memo-carried loops, string growth,
// `return set(..)` through a helper, a child rendered twice, setter aliases.
// Constants stay below 4 so a guarded program settles in a handful of renders.
// The world is open on two sides: a program may span several files, and a
// write may go through `ext`, a package the analyzer has no source for.

export function rng(seed) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const r = {
    next,
    int: (lo, hi) => lo + Math.floor(next() * (hi - lo + 1)),
    chance: (p) => next() < p,
    pick: (xs) => xs[Math.floor(next() * xs.length)],
    weighted: (pairs) => {
      const total = pairs.reduce((s, [w]) => s + w, 0);
      if (total === 0) return null;
      let x = next() * total;
      for (const [w, v] of pairs) if ((x -= w) < 0) return v;
      return pairs[pairs.length - 1][1];
    },
  };
  return r;
}

const num = (v) => ({ kind: "num", v });
const v = (name) => ({ kind: "var", name });

/** Generates program number `index` of the stream `seed`. */
export function generate(seed, index) {
  const r = rng(seed * 1_000_003 + index);
  const p = { kind: "program", helpers: [], comps: [] };
  const nComps = r.weighted([[5, 1], [4, 2], [2, 3]]);
  for (let i = 0; i < nComps; i++) {
    const isApp = i === nComps - 1;
    p.comps.push(genComp(r, p, isApp ? "App" : `C${i}`, isApp));
  }
  // Some programs are split across files that import each other: the module
  // helpers in `lib.tsx`, the first components in a file each. A component
  // renders only earlier ones, so moving out a prefix never makes a file import
  // `App.tsx`. The shrinker can fold anything back into it.
  if (r.chance(0.3)) {
    for (const h of p.helpers) h.file = "lib.tsx";
    const cut = r.int(0, nComps - 1);
    for (const [i, c] of p.comps.entries()) c.file = i < cut ? `${c.name}.tsx` : "App.tsx";
  }
  return p;
}

function helperName(p, base) {
  return `${base}${p.helpers.filter((h) => h.name.startsWith(base)).length}`;
}

function genComp(r, p, name, isApp) {
  const comp = { kind: "comp", name, exported: isApp || r.chance(0.5), props: [], body: [], view: null };
  // env.reads: [{ e: Expr, type }]; env.writers: [{ name, type, kind }]
  const env = { reads: [], writers: [], refs: [], fns: [], counter: 0 };
  if (!isApp) {
    const n = r.int(0, 2);
    for (let i = 0; i < n; i++) {
      const type = r.weighted([[4, "num"], [2, "fn"], [1, "obj"], [1, "str"]]);
      const pname = type === "fn" ? `on${i}` : `p${i}`;
      comp.props.push({ name: pname, type });
      if (type === "fn") env.writers.push({ name: pname, type: "num", kind: "propfn" });
      else env.reads.push({ e: v(pname), type });
    }
  }

  const nStates = isApp ? r.int(1, 3) : r.int(0, 2);
  for (let i = 0; i < nStates; i++) addState(r, p, comp, env, i);
  if (r.chance(0.15)) addReducer(r, p, comp, env);
  if (r.chance(0.15)) {
    const name = `r${env.refs.length}`;
    comp.body.push({ kind: "ref", name, init: num(0) });
    env.refs.push(name);
    env.reads.push({ e: { kind: "member", obj: v(name), prop: "current" }, type: "num" });
  }
  if (r.chance(0.2)) {
    const type = r.pick(["num", "obj"]);
    const name = `m${comp.body.length}`;
    comp.body.push({ kind: "memo", name, body: genExpr(r, env, type, 2), deps: genDeps(r, env, false) });
    env.reads.push({ e: v(name), type });
  }
  if (r.chance(0.15)) {
    const name = `obj${comp.body.length}`;
    comp.body.push({ kind: "local", name, value: genExpr(r, env, "obj", 1, true) });
    env.reads.push({ e: v(name), type: "obj" });
  }
  if (r.chance(0.15) && env.writers.some((w) => w.kind === "setter")) {
    const target = r.pick(env.writers.filter((w) => w.kind === "setter"));
    const name = `alias${comp.body.length}`;
    comp.body.push({ kind: "alias", name, of: target.name });
    env.writers.push({ ...target, name, kind: "setter" });
  }
  if (r.chance(0.15) && env.writers.length) {
    const name = `cb${comp.body.length}`;
    const inner = { ...env, reads: [...env.reads, { e: v("x"), type: "num" }] };
    const body = genStmts(r, p, inner, "callback", r.int(1, 2), 1);
    comp.body.push({ kind: "callback", name, param: "x", body, deps: genDeps(r, env, false) });
    env.fns.push(name);
  }
  // A render-phase write, guarded or not; rare, it is the loudest shape.
  if (r.chance(0.12) && env.writers.length) {
    const w = genWrite(r, p, env, "render");
    comp.body.push(r.chance(0.7) ? { kind: "if", cond: genExpr(r, env, "bool", 1), then: [w], else: [] } : w);
  }
  const nEffects = r.weighted([[2, 0], [5, 1], [3, 2], [1, 3]]);
  for (let i = 0; i < nEffects; i++) {
    const body = genStmts(r, p, env, "effect", r.int(1, 2), 2);
    const cleanup = r.chance(0.15) ? genStmts(r, p, env, "cleanup", 1, 1) : null;
    comp.body.push({ kind: "effect", layout: r.chance(0.15), body, cleanup, deps: genDeps(r, env, true) });
  }
  comp.view = genView(r, p, comp, env);
  return comp;
}

function addState(r, p, comp, env, i) {
  const type = r.weighted([[6, "num"], [2, "obj"], [1, "str"], [1, "bool"], [1, "arr"]]);
  const name = `s${i}`;
  const setter = `setS${i}`;
  const init = initFor(r, type);
  if (type === "num" && r.chance(0.1)) {
    let hook = p.helpers.find((h) => h.kind === "hook");
    if (!hook) p.helpers.unshift((hook = { kind: "hook", name: "useSlot" }));
    comp.body.push({ kind: "custom", hook: hook.name, name, setter, init });
  } else {
    comp.body.push({ kind: "state", name, setter, init, lazy: r.chance(0.05) });
  }
  env.reads.push({ e: v(name), type });
  env.writers.push({ name: setter, type, kind: "setter", state: name });
}

function addReducer(r, p, comp, env) {
  const type = r.pick(["num", "obj"]);
  const name = helperName(p, "reduce");
  const s = v("s");
  const a = v("a");
  let body;
  if (type === "num") {
    body = r.pick([
      { kind: "bin", op: "+", l: s, r: a },
      { kind: "cond", c: { kind: "bin", op: "===", l: a, r: num(0) }, t: s, f: { kind: "bin", op: "+", l: s, r: a } },
      { kind: "fcall", fn: "Math.min", args: [{ kind: "bin", op: "+", l: s, r: a }, num(3)] },
    ]);
  } else {
    const bumped = { kind: "obj", spread: s, fields: [["v", { kind: "bin", op: "+", l: { kind: "member", obj: s, prop: "v" }, r: a }]] };
    body = r.pick([bumped, { kind: "cond", c: { kind: "bin", op: "===", l: a, r: num(0) }, t: s, f: bumped }, s]);
  }
  p.helpers.push({ kind: "reducer", name, body });
  const sname = `rs${comp.body.length}`;
  const dispatch = `dispatch${comp.body.length}`;
  comp.body.push({ kind: "reducer", name: sname, dispatch, reducer: name, init: type === "num" ? num(0) : { kind: "obj", spread: null, fields: [["v", num(0)]] } });
  env.reads.push({ e: v(sname), type });
  env.writers.push({ name: dispatch, type: "num", kind: "dispatch" });
}

function initFor(r, type) {
  switch (type) {
    case "num":
      return num(r.int(0, 2));
    case "str":
      return { kind: "str", v: r.pick(["", "a"]) };
    case "bool":
      return { kind: "bool", v: r.chance(0.5) };
    case "obj":
      return { kind: "obj", spread: null, fields: [["v", num(0)]] };
    case "arr":
      return { kind: "arr", spread: null, items: [] };
  }
}

function genDeps(r, env, allowNone) {
  const shape = r.weighted([[allowNone ? 3 : 0, "none"], [2, "empty"], [5, "some"]]);
  if (shape === "none") return null;
  if (shape === "empty" || !env.reads.length) return [];
  const n = r.int(1, Math.min(2, env.reads.length));
  const picked = new Set();
  for (let i = 0; i < n; i++) picked.add(r.pick([...env.reads.map((x) => x.e), ...env.fns.map(v)]));
  return [...picked].filter((e) => e.kind === "var" || e.kind === "member");
}

function genStmts(r, p, env, where, n, depth) {
  const out = [];
  for (let i = 0; i < n; i++) {
    const s = genStmt(r, p, env, where, depth);
    if (s) out.push(s);
  }
  return out;
}

function genStmt(r, p, env, where, depth) {
  const choice = r.weighted([
    [env.writers.length ? 8 : 0, "write"],
    [depth > 0 ? 2 : 0, "if"],
    [env.refs.length ? 1 : 0, "refset"],
    [env.fns.length ? 1 : 0, "call"],
  ]);
  switch (choice) {
    case "write":
      return genWrite(r, p, env, where);
    case "if":
      return {
        kind: "if",
        cond: genExpr(r, env, "bool", 1),
        then: genStmts(r, p, env, where, 1, depth - 1),
        else: r.chance(0.3) ? genStmts(r, p, env, where, 1, depth - 1) : [],
      };
    case "refset": {
      const ref = r.pick(env.refs);
      return { kind: "refset", ref, value: genExpr(r, env, "num", 1) };
    }
    case "call":
      return { kind: "call", target: r.pick(env.fns), arg: genExpr(r, env, "num", 1) };
    default:
      return null;
  }
}

function genWrite(r, p, env, where) {
  const w = r.pick(env.writers);
  let arg;
  if (w.kind === "setter" && r.chance(0.3)) {
    const inner = { ...env, reads: [...env.reads, { e: v("prev"), type: w.type }] };
    arg = { kind: "updater", param: "prev", body: genExpr(r, inner, w.type, 2, false, "prev") };
  } else {
    arg = genExpr(r, env, w.type, 2);
  }
  const async = where === "effect" || where === "callback";
  const wrap = r.weighted([
    [10, "plain"],
    [2, "and"],
    [2, "ternary"],
    [async ? 2 : 0, "timeout"],
    [async ? 2 : 0, "then"],
    [1, "iife"],
    // Through a package the analyzer cannot see into (ADR-009's `Unknown`).
    [1, "runNow"],
    [async ? 1 : 0, "defer"],
    [w.type === "num" && w.kind === "setter" && arg.kind !== "updater" ? 2 : 0, "helper"],
  ]);
  if (wrap === "helper") {
    let h = p.helpers.find((x) => x.kind === "bump" && r.chance(0.5));
    if (!h) {
      const form = r.pick(["call", "return", "arrow"]);
      h = { kind: "bump", name: helperName(p, "bump"), form, arg: genExpr(r, { reads: [{ e: v("n"), type: "num" }] }, "num", 1, false, "n") };
      p.helpers.push(h);
    }
    return { kind: "helpercall", helper: h.name, target: w.name, arg: genExpr(r, env, "num", 1) };
  }
  const s = { kind: "write", target: w.name, arg, wrap };
  if (wrap === "and" || wrap === "ternary") s.cond = genExpr(r, env, "bool", 1);
  return s;
}

/** A typed expression over `env.reads`; `prefer` biases toward one variable. */
function genExpr(r, env, type, depth, fresh = false, prefer = null) {
  const reads = env.reads.filter((x) => x.type === type);
  const preferred = prefer ? reads.filter((x) => x.e.kind === "var" && x.e.name === prefer) : [];
  if (preferred.length && r.chance(0.6) && depth > 0) {
    // Build on the preferred variable: `prev + 1`, `{ ...prev, v: .. }`.
    return grow(r, env, type, preferred[0].e, depth);
  }
  const leaf = () => (reads.length && r.chance(0.6) && !fresh ? r.pick(reads).e : literal(r, env, type, depth));
  if (depth <= 0) return leaf();
  const kind = r.weighted([[4, "leaf"], [4, "grow"], [type === "num" || type === "bool" ? 1 : 0, "cond"]]);
  if (kind === "leaf") return leaf();
  if (kind === "cond") return { kind: "cond", c: genExpr(r, env, "bool", depth - 1), t: genExpr(r, env, type, depth - 1), f: genExpr(r, env, type, depth - 1) };
  return grow(r, env, type, reads.length && !fresh ? r.pick(reads).e : literal(r, env, type, 0), depth);
}

function grow(r, env, type, base, depth) {
  switch (type) {
    case "num":
      return r.weighted([
        [5, { kind: "bin", op: "+", l: base, r: num(r.int(1, 2)) }],
        [1, { kind: "bin", op: "*", l: base, r: num(2) }],
        [1, { kind: "bin", op: "%", l: base, r: num(3) }],
        [2, { kind: "fcall", fn: "Math.min", args: [{ kind: "bin", op: "+", l: base, r: num(1) }, num(3)] }],
        [1, { kind: "bin", op: "-", l: base, r: genExpr(r, env, "num", depth - 1) }],
      ]);
    case "str":
      return { kind: "bin", op: "+", l: base, r: { kind: "str", v: "x" } };
    case "bool":
      return r.weighted([
        [3, { kind: "bin", op: r.pick(["<", ">", "==="]), l: genExpr(r, env, "num", depth - 1), r: num(r.int(0, 3)) }],
        [1, { kind: "not", e: base }],
        [1, { kind: "bin", op: r.pick(["===", "!=="]), l: base, r: genExpr(r, env, "bool", 0) }],
      ]);
    case "obj":
      return r.weighted([
        [3, { kind: "obj", spread: base, fields: [["v", genExpr(r, env, "num", depth - 1)]] }],
        [2, { kind: "obj", spread: null, fields: [["v", genExpr(r, env, "num", depth - 1)]] }],
        [1, base],
      ]);
    case "arr":
      return r.weighted([
        [3, { kind: "arr", spread: base, items: [genExpr(r, env, "num", depth - 1)] }],
        [1, { kind: "arr", spread: null, items: [] }],
        [1, base],
      ]);
  }
}

function literal(r, env, type, depth) {
  switch (type) {
    case "num": {
      const derived = env.reads.filter((x) => x.type === "obj" || x.type === "arr" || x.type === "str");
      if (derived.length && r.chance(0.2)) {
        const d = r.pick(derived);
        return { kind: "member", obj: d.e, prop: d.type === "obj" ? "v" : "length" };
      }
      return num(r.int(0, 3));
    }
    case "str":
      return { kind: "str", v: r.pick(["", "a", "x"]) };
    case "bool":
      return { kind: "bool", v: r.chance(0.5) };
    case "obj":
      return { kind: "obj", spread: null, fields: [["v", depth > 0 ? genExpr(r, env, "num", depth - 1) : num(0)]] };
    case "arr":
      return { kind: "arr", spread: null, items: [] };
  }
}

function genView(r, p, comp, env) {
  const children = [];
  const shown = env.reads.filter((x) => x.type === "num" || x.type === "str");
  if (shown.length) children.push({ kind: "text", e: r.pick(shown).e });
  const below = p.comps; // components declared so far
  const n = below.length ? r.weighted([[2, 0], [4, 1], [2, 2]]) : 0;
  for (let i = 0; i < n; i++) {
    const child = r.pick(below);
    const props = child.props.map((pr) => [pr.name, propValue(r, env, pr.type)]);
    children.push({
      kind: "child",
      comp: child.name,
      props,
      key: r.chance(0.1) ? genExpr(r, env, "num", 1) : null,
      cond: r.chance(0.15) ? genExpr(r, env, "bool", 1) : null,
    });
  }
  return { kind: "el", tag: "div", children };
}

function propValue(r, env, type) {
  if (type !== "fn") return genExpr(r, env, type, 1);
  const setters = env.writers.filter((w) => w.type === "num" && w.kind !== "propfn");
  const passthrough = env.writers.filter((w) => w.kind === "propfn");
  const c = r.weighted([[setters.length ? 4 : 0, "setter"], [setters.length ? 3 : 0, "arrow"], [env.fns.length ? 1 : 0, "cb"], [passthrough.length ? 1 : 0, "pass"], [1, "noop"]]);
  switch (c) {
    case "setter":
      return v(r.pick(setters).name);
    case "arrow": {
      const w = r.pick(setters);
      const inner = { ...env, reads: [...env.reads, { e: v("x"), type: "num" }] };
      return { kind: "arrow", param: "x", body: { kind: "write", target: w.name, arg: genExpr(r, inner, "num", 1, false, "x"), wrap: "plain" } };
    }
    case "cb":
      return v(r.pick(env.fns));
    case "pass":
      return v(r.pick(passthrough).name);
    default:
      return { kind: "arrow", param: "x", body: { kind: "num", v: 0 } };
  }
}
