// The concrete semantics: one program rendered by real React under jsdom, with
// no user input, and the trace reduced to a verdict (docs/semantics.md §3).
//
// The source is compiled as written and run unchanged. Two things are
// instrumented, neither visible to the program: the JSX runtime wraps every
// function component to count its renders, and the timer functions the program
// sees are a virtual clock the runner drains one callback at a time.

import { createRequire } from "node:module";
import { JSDOM } from "jsdom";
import { transformSync } from "esbuild";
import { splitFiles, ENTRY } from "./files.mjs";

const require = createRequire(import.meta.url);

// A program that renders more than this many times in one step — the mount, or
// one timer callback — without React stopping it first, is read as a loop:
// it renders without time advancing. React-tRace's conformance suite uses 100
// re-renders for the same purpose; the generated programs settle in a few
// dozen at most, so the gap is wide.
export const RENDER_BUDGET = 300;
// Timer callbacks fired before the run is read as perpetual: it keeps
// re-arming timers, so it never settles, but at the pace of the clock (§3).
export const TIMER_BUDGET = 200;
// A component rendered at least this many times in the step React stopped is
// taking part in the loop (P1 asks for a claim on one of them). The cycle's
// components render dozens of times before any guard fires (25, 50, or the
// budget); an ancestor React re-renders once to retry after the error does not.
export const IN_LOOP = 10;

/**
 * A package the analyzer cannot see into (ADR-009's `Unknown` callee): a bare
 * specifier that resolves nowhere on disk. `runNow` calls its callback at once,
 * `defer` in a microtask, like `Promise.then`; neither is a timer. `format` is
 * a function of its argument, which the analyzer cannot know (#224).
 * `useRun(x)` is a hook from that package: after every commit it calls `x`,
 * or each function member of `x` (#254).
 */
const EXT = {
  runNow: (cb) => cb(),
  defer: (cb) => void Promise.resolve().then(cb),
  format: (x) => `<${x}>`,
  useRun: (x) =>
    React.useEffect(() => {
      const fns = typeof x === "function" ? [x] : Object.values(x ?? {});
      for (const f of fns) if (typeof f === "function") f();
    }),
};

const dom = new JSDOM("<!doctype html><html><body></body></html>");
for (const key of ["window", "document", "navigator", "HTMLElement", "Node", "Element"]) {
  const value = key === "window" ? dom.window : dom.window[key];
  Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const React = require("react");
const jsxRuntime = require("react/jsx-runtime");
const ReactDOMClient = require("react-dom/client");

class RenderBudgetExceeded extends Error {
  constructor() {
    super(`oracle: more than ${RENDER_BUDGET} renders in one step`);
    this.name = "RenderBudgetExceeded";
  }
}

/** Per-run state, reset by `runProgram`. */
let run = null;
/** Counts the runs of this worker; see `counted`. */
let runs = 0;

const wrappers = new WeakMap();
function counted(type) {
  if (typeof type !== "function" || (type.prototype && type.prototype.isReactComponent)) return type;
  let w = wrappers.get(type);
  if (!w) {
    const name = type.displayName || type.name || "Anonymous";
    // React warns once per component name for the life of the process
    // ("Cannot update a component while rendering a different component"),
    // and a worker runs many programs: the name React sees is made unique
    // to the run, so no program's warnings depend on the ones before it.
    const displayName = `${name}#${runs}`;
    w = function (props) {
      if (run) {
        run.renders[name] = (run.renders[name] ?? 0) + 1;
        run.stepRenders[name] = (run.stepRenders[name] ?? 0) + 1;
        run.total += 1;
        if (++run.step > RENDER_BUDGET) throw new RenderBudgetExceeded();
      }
      return type(props);
    };
    Object.defineProperty(w, "name", { value: name });
    w.displayName = displayName;
    wrappers.set(type, w);
  }
  return w;
}

const jsxShim = {
  Fragment: jsxRuntime.Fragment,
  jsx: (type, props, key) => jsxRuntime.jsx(counted(type), props, key),
  jsxs: (type, props, key) => jsxRuntime.jsxs(counted(type), props, key),
};
// `React.createElement` is reached by programs that build elements by hand.
const reactShim = new Proxy(React, {
  get(target, prop) {
    if (prop === "createElement") return (type, ...rest) => React.createElement(counted(type), ...rest);
    return target[prop];
  },
});

function virtualClock() {
  let now = 0;
  let seq = 0;
  const queue = new Map(); // id → { at, seq, fn, args, every }
  const add = (fn, delay, args, every) => {
    const id = ++seq;
    const d = Math.max(0, Number(delay) || 0);
    queue.set(id, { at: now + d, seq: id, fn, args, every: every ? Math.max(1, d) : null });
    return id;
  };
  return {
    api: {
      setTimeout: (fn, delay, ...args) => add(fn, delay, args, false),
      clearTimeout: (id) => void queue.delete(id),
      setInterval: (fn, delay, ...args) => add(fn, delay, args, true),
      clearInterval: (id) => void queue.delete(id),
      requestAnimationFrame: (fn) => add(() => fn(now), 16, [], false),
      cancelAnimationFrame: (id) => void queue.delete(id),
    },
    pending: () => queue.size,
    /** Removes the earliest timer and returns a thunk that fires it. */
    next() {
      let best = null;
      for (const [id, t] of queue) if (!best || t.at < best[1].at || (t.at === best[1].at && t.seq < best[1].seq)) best = [id, t];
      if (!best) return null;
      const [id, t] = best;
      now = t.at;
      if (t.every) t.at = now + t.every;
      else queue.delete(id);
      return () => t.fn(...t.args);
    },
  };
}

const LOOP_PATTERNS = [
  [/Too many re-renders/, "render-phase"],
  [/Maximum update depth exceeded/, "update-depth"],
];
const CROSS_RENDER_UPDATE = /Cannot update a component .* while rendering a different component/s;

function classifyError(err) {
  if (err instanceof RenderBudgetExceeded || err?.name === "RenderBudgetExceeded") return { verdict: "loop", via: "render-budget" };
  const msg = String(err?.message ?? err);
  for (const [re, via] of LOOP_PATTERNS) if (re.test(msg)) return { verdict: "loop", via };
  return { verdict: "crash", via: msg.split("\n")[0].slice(0, 200) };
}

export function compile(source, name = ENTRY) {
  return transformSync(source, {
    loader: "tsx",
    jsx: "automatic",
    format: "cjs",
    target: "es2022",
    sourcefile: name,
  }).code;
}

/**
 * Evaluates every file of `source` as a CommonJS module, the entry first;
 * `./X` resolves to the file `X.tsx` (or `.ts`) of the same program.
 */
function load(source, clock) {
  const files = new Map(splitFiles(source).map((f) => [f.name, f.text]));
  const cache = new Map();
  const t = clock.api;
  const evaluate = (name) => {
    if (cache.has(name)) return cache.get(name).exports;
    const module = { exports: {} };
    cache.set(name, module);
    new Function(
      "require", "module", "exports",
      "setTimeout", "clearTimeout", "setInterval", "clearInterval", "requestAnimationFrame", "cancelAnimationFrame",
      compile(files.get(name), name),
    )(req, module, module.exports, t.setTimeout, t.clearTimeout, t.setInterval, t.clearInterval, t.requestAnimationFrame, t.cancelAnimationFrame);
    return module.exports;
  };
  const req = (id) => {
    if (id === "react") return reactShim;
    if (id === "react/jsx-runtime") return jsxShim;
    if (id === "react-dom") return require("react-dom");
    if (id === "react-dom/client") return ReactDOMClient;
    if (id === "ext") return EXT;
    if (id.startsWith("./")) {
      const base = id.slice(2);
      const name = [base, `${base}.tsx`, `${base}.ts`].find((n) => files.has(n));
      if (name) return evaluate(name);
    }
    throw new Error(`oracle: cannot import ${id}`);
  };
  return evaluate(files.has(ENTRY) ? ENTRY : files.keys().next().value);
}

/**
 * Renders `root` (default `App`) from `source` and returns the verdict:
 * `loop` (React stopped it, or it rendered past the budget without time
 * advancing), `perpetual` (it kept re-arming timers past the timer budget),
 * `converges` (no pending work left), or `crash` (any other error: the program
 * is not a valid test). `inLoop` names the components the loop re-renders.
 */
export async function runProgram(source, { root = "App" } = {}) {
  const clock = virtualClock();
  const errors = [];
  const warnings = [];
  runs += 1;
  run = { renders: {}, total: 0, step: 0, stepRenders: {} };

  const origError = console.error;
  const origWarn = console.warn;
  console.error = (...args) => warnings.push(args.map(String).join(" "));
  console.warn = () => {};

  const finish = (verdict, via) => {
    console.error = origError;
    console.warn = origWarn;
    const inLoop = verdict === "loop" ? Object.keys(run.stepRenders).filter((k) => run.stepRenders[k] >= IN_LOOP).sort() : [];
    const out = { verdict, via, renders: run.renders, total: run.total, inLoop, warnings: summarizeWarnings(warnings) };
    run = null;
    return out;
  };

  let exports;
  try {
    exports = load(source, clock);
  } catch (e) {
    return finish("crash", `compile/eval: ${String(e?.message ?? e).split("\n")[0]}`);
  }
  const App = exports[root];
  if (typeof App !== "function") return finish("crash", `no exported component \`${root}\``);

  const container = document.createElement("div");
  const reactRoot = ReactDOMClient.createRoot(container, {
    onUncaughtError: (e) => errors.push(e),
    onCaughtError: (e) => errors.push(e),
    onRecoverableError: () => {},
  });

  const verdictFromState = () => {
    for (const e of errors) {
      const c = classifyError(e);
      if (c.verdict === "loop") return c;
    }
    for (const w of warnings) for (const [re, via] of LOOP_PATTERNS) if (re.test(w)) return { verdict: "loop", via };
    if (run.step > RENDER_BUDGET) return { verdict: "loop", via: "render-budget" };
    if (errors.length) return classifyError(errors[0]);
    return null;
  };

  try {
    try {
      await React.act(async () => reactRoot.render(jsxShim.jsx(App, {})));
    } catch (e) {
      errors.push(e);
    }
    let fired = 0;
    for (;;) {
      const v = verdictFromState();
      if (v) return finish(v.verdict, v.via);
      const fire = clock.next();
      if (!fire) break;
      if (++fired > TIMER_BUDGET) return finish("perpetual", "timer-budget");
      run.step = 0;
      run.stepRenders = {};
      try {
        await React.act(async () => fire());
      } catch (e) {
        errors.push(e);
      }
    }
    return finish("converges", null);
  } finally {
    try {
      await React.act(async () => reactRoot.unmount());
    } catch {
      // A root that already failed has nothing left to unmount.
    }
  }
}

function summarizeWarnings(ws) {
  const out = new Set();
  for (const w of ws) {
    if (CROSS_RENDER_UPDATE.test(w)) out.add("cross-render-update");
    else if (/Maximum update depth exceeded/.test(w)) out.add("update-depth");
  }
  return [...out];
}
