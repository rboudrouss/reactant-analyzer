// The programmatic API (lib/index.js, lib/browser.js).
//
// The load-bearing assertion is the first one: `analyzeProject` and
// `check --format json` must produce the same document, because they are
// supposed to be the same call with a different reporter. Everything after
// it covers the surface the CLI never exercises — a virtual file map (the
// browser's only mode), thrown usage errors, and the guarantee that the
// browser entry's module graph stays free of `node:` imports.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import {
  analyze,
  analyzeProject,
  explain,
  hostConstants,
  rules,
  run,
  UsageError,
  validatePack,
} from "../lib/index.js";

// Fixture paths, and the paths the reports display, are repo-relative: the
// CLI comparison only holds from the same cwd.
process.chdir(path.join(import.meta.dirname, "..", ".."));

const VITE = "tests/fixtures/vite_project";
const PACKS = "tests/fixtures/pack_project";

/** An effect that writes the state its own deps gate: one infinite-loop. */
const LOOP = [
  'import { useEffect, useState } from "react";',
  "export function App() {",
  "  const [n, setN] = useState(0);",
  "  useEffect(() => { setN(n + 1); }, [n]);",
  "  return <div>{n}</div>;",
  "}",
].join("\n");

let failed = 0;
async function test(name, body) {
  try {
    await body();
    console.log(`ok:   ${name}`);
  } catch (e) {
    console.log(`FAIL: ${name}\n      ${e.message.split("\n").slice(0, 6).join("\n      ")}`);
    failed = 1;
  }
}

/** The CLI's own JSON document, for the same tree and the same cwd. */
function cli(...args) {
  return JSON.parse(
    execFileSync(process.execPath, ["npm/bin/reactant.js", ...args], {
      encoding: "utf8",
      env: { ...process.env, NO_COLOR: "1" },
      stdio: ["ignore", "pipe", "ignore"],
    }),
  );
}

await test("analyzeProject == check --format json", async () => {
  const { report, exitCode } = await analyzeProject([VITE], { failOn: "never" });
  assert.deepStrictEqual(report, cli("check", VITE, "--format", "json", "--fail-on", "never"));
  assert.equal(exitCode, 0);
});

await test("analyzeProject carries options through (--info)", async () => {
  const { report } = await analyzeProject([VITE], { info: true, failOn: "never" });
  const expected = cli("check", VITE, "--format", "json", "--info", "--fail-on", "never");
  assert.deepStrictEqual(report, expected);
  assert.ok(report.summary.infos > 0, "the fixture has infos to report under --info");
});

await test("analyzeProject resolves the packs a config names", async () => {
  const { report } = await analyzeProject([PACKS], { failOn: "never" });
  assert.deepStrictEqual(report, cli("check", PACKS, "--format", "json", "--fail-on", "never"));
  assert.ok(
    report.diagnostics.some((d) => d.rule.startsWith("team/")),
    "a pack rule fired",
  );
});

await test("analyzeProject honors failOn in the exit code", async () => {
  const { exitCode } = await analyzeProject([VITE], { failOn: "warning" });
  assert.equal(exitCode, 1);
});

await test("analyze over a virtual file map (no filesystem)", async () => {
  const { report } = await analyze({
    files: { "package.json": '{"name":"demo"}', "src/App.tsx": LOOP },
    failOn: "never",
  });
  const [d] = report.diagnostics;
  assert.equal(report.files_analyzed, 1);
  assert.equal(d.rule, "infinite-loop");
  assert.equal(d.file, "src/App.tsx");
  assert.equal(d.line, 4);
  assert.ok(d.notes.length > 0, "the witness chain came through");
});

await test("analyze takes a config object and applies it", async () => {
  const files = { "App.tsx": LOOP };
  const on = await analyze({ files, failOn: "never" });
  assert.deepStrictEqual(
    on.report.diagnostics.map((d) => d.rule),
    ["infinite-loop"],
  );
  const off = await analyze({
    files,
    config: { rules: { "infinite-loop": "off" } },
    failOn: "never",
  });
  assert.deepStrictEqual(off.report.diagnostics, [], "the config turned the rule off");
});

await test("analyze takes a pack inline", async () => {
  const pack = JSON.parse(fs.readFileSync("npm/test/fixtures/team.pack.expected.json", "utf8"));
  const { name, rules: ids } = await validatePack(pack);
  assert.equal(name, "team");
  const { report } = await analyze({
    files: { "App.tsx": "export function App() { return null; }" },
    packs: [{ name: "team", json: pack }],
    failOn: "never",
  });
  assert.equal(report.version, 2);
  const listed = (await rules({ packs: [{ name: "team", json: pack }] })).text;
  for (const id of ids) assert.ok(listed.includes(id), `${id} is listed`);
});

await test("run returns the human report the CLI prints", async () => {
  const out = await run({
    command: "check",
    files: { "App.tsx": "export function App() { return null; }" },
    format: "human",
    failOn: "never",
  });
  assert.equal(out.exitCode, 0);
  assert.match(out.stdout, /component/);
});

await test("explain and rules render text", async () => {
  assert.match((await explain("infinite-loop")).text, /infinite-loop/);
  assert.match((await rules()).text, /infinite-loop/);
});

await test("a rejected rule name throws UsageError", async () => {
  await assert.rejects(() => explain("no-such-rule"), UsageError);
});

await test("an unknown option throws UsageError", async () => {
  await assert.rejects(() => analyze({ files: {}, infoo: true }), UsageError);
  await assert.rejects(() => analyze({ files: {}, format: "human" }), UsageError);
  await assert.rejects(() => analyze({ files: { "a.tsx": 42 } }), UsageError);
});

await test("a broken config throws UsageError, not a report", async () => {
  await assert.rejects(() => analyze({ files: {}, config: "{ not json" }), UsageError);
});

await test("analyzeProject refuses inputs it reads from disk", async () => {
  await assert.rejects(() => analyzeProject([VITE], { files: {} }), UsageError);
  await assert.rejects(() => analyzeProject([VITE], { config: "{}" }), UsageError);
});

await test("hostConstants is served by the core", async () => {
  const c = await hostConstants();
  assert.equal(c.configFileName, "reactant.config.json");
  assert.ok(c.sourceExtensions.includes("tsx"));
});

// A `node:` import anywhere in the browser entry's graph would break every
// bundler that resolves the `browser` condition — and would do it at the
// consumer's build, not here.
await test("the browser entry's module graph is host-free", async () => {
  const seen = new Set();
  const queue = ["npm/lib/browser.js"];
  while (queue.length) {
    const file = queue.pop();
    if (seen.has(file) || !fs.existsSync(file)) continue;
    seen.add(file);
    const source = fs.readFileSync(file, "utf8");
    for (const [, spec] of source.matchAll(/^\s*import\s[^;]*?["']([^"']+)["']/gm)) {
      assert.ok(!spec.startsWith("node:"), `${file} imports ${spec}`);
      if (spec.startsWith(".")) queue.push(path.join(path.dirname(file), spec));
    }
  }
  assert.ok(seen.size >= 4, `walked the graph (${seen.size} files)`);
});

process.exitCode = failed;
