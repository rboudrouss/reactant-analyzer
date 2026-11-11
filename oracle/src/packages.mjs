// The real packages a program may import (`import { … } from "react-router"`):
// the `dependencies` of oracle/package.json, minus the ones the runner itself
// is made of. Adding a library to a case is adding it to package.json (pinned
// to an exact version); nothing else lists it.

import { readFileSync } from "node:fs";

const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));

/** The runner's own dependencies: React is provided (and instrumented) by `react-run.mjs`. */
const RUNNER = new Set(["react", "react-dom", "esbuild", "jsdom"]);

export const PACKAGES = new Set(Object.keys(manifest.dependencies ?? {}).filter((name) => !RUNNER.has(name)));

/** The importable package an import specifier names (`pkg`, `pkg/sub`, `@scope/pkg/sub`), or null. */
export function packageOf(id) {
  const parts = id.split("/");
  const name = id.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
  return PACKAGES.has(name) ? name : null;
}
