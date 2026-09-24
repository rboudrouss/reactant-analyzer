// Render-count bench: bundles a scenario .tsx, mounts it in jsdom, resets the
// counters, performs the scenario's interaction, prints renders per component.
// Instrumentation is injected at build time (an esbuild plugin inserts
// `__track("Name")` at the top of every capitalised function), so the scenario
// files stay clean enough to feed to reactant unchanged.
import { build, transform } from "esbuild";
import { JSDOM } from "jsdom";
import { readFileSync } from "node:fs";
import path from "node:path";

const file = path.resolve(process.argv[2]);

const trackPlugin = {
  name: "track",
  setup(b) {
    b.onLoad({ filter: /scenarios\/.*\.tsx$/ }, async (args) => {
      // Strip types first so parameter lists have no nested parens.
      let src = (await transform(readFileSync(args.path, "utf8"), { loader: "tsx", jsx: "preserve" })).code;
      // function Foo(...) {   and   const Foo = (...) => {   and   memo(function Foo(...) {
      src = src.replace(
        /function ([A-Z]\w*)\s*\(([^)]*)\)\s*\{/g,
        (m, name) => `${m} __track(${JSON.stringify(name)});`,
      );
      src = src.replace(
        /const ([A-Z]\w*)\s*=\s*\(([^)]*)\)\s*=>\s*\{/g,
        (m, name) => `${m} __track(${JSON.stringify(name)});`,
      );
      return { contents: src, loader: "jsx" };
    });
  },
};

const out = await build({
  entryPoints: [file],
  bundle: true,
  write: false,
  format: "esm",
  platform: "node",
  jsx: "automatic",
  external: ["react", "react-dom", "react-dom/client"],
  plugins: [trackPlugin],
});

const dom = new JSDOM("<!doctype html><div id=root></div>", { pretendToBeVisual: true });
Object.assign(globalThis, {
  window: dom.window,
  document: dom.window.document,
  HTMLElement: dom.window.HTMLElement,
  Node: dom.window.Node,
  MouseEvent: dom.window.MouseEvent,
  Event: dom.window.Event,
  IS_REACT_ACT_ENVIRONMENT: true,
});

const counts = new Map();
globalThis.__track = (n) => counts.set(n, (counts.get(n) ?? 0) + 1);

const code = out.outputFiles[0].text;
const { writeFileSync, mkdirSync } = await import("node:fs");
mkdirSync(".out", { recursive: true });
const outFile = path.resolve(".out", path.basename(file, ".tsx") + ".mjs");
writeFileSync(outFile, code);
const mod = await import(outFile);
const React = await import("react");
const { createRoot } = await import("react-dom/client");

const root = createRoot(document.getElementById("root"));
await React.act(async () => root.render(React.createElement(mod.default)));
const mounted = new Map(counts);
counts.clear();

const $ = (sel) => {
  const el = document.querySelector(sel);
  if (!el) throw new Error(`no element ${sel}`);
  return el;
};
const ui = {
  async type(sel, text) {
    const el = $(sel);
    const proto = Object.getPrototypeOf(el);
    const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
    for (const ch of text) {
      await React.act(async () => {
        setter.call(el, el.value + ch);
        el.dispatchEvent(new window.Event("input", { bubbles: true }));
      });
    }
  },
  async click(sel) {
    await React.act(async () => $(sel).dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
  },
  async fire(sel, type, init = {}) {
    await React.act(async () => $(sel).dispatchEvent(new window.MouseEvent(type, { bubbles: true, ...init })));
  },
  async fireWindow(type, times = 1) {
    for (let i = 0; i < times; i++)
      await React.act(async () => window.dispatchEvent(new window.MouseEvent(type, { clientX: i, clientY: i })));
  },
};
await mod.interact(ui);

const names = [...new Set([...mounted.keys(), ...counts.keys()])];
const label = mod.interaction ?? "interaction";
console.log(`${path.basename(file)}: ${label}`);
for (const n of names) {
  const c = counts.get(n) ?? 0;
  console.log(`  ${n.padEnd(18)} mount=${mounted.get(n) ?? 0}  after=${c}${c === 0 ? "" : ""}`);
}
await React.act(async () => root.unmount());
