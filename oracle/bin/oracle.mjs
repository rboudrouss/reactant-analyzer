#!/usr/bin/env node
// reactant-oracle — checks the analyzer against real React (docs/semantics.md).
//
//   oracle.mjs run FILE...                 judge hand-written programs, print everything
//   oracle.mjs cases [DIR]                 the regression suite: oracle/cases/*.tsx
//   oracle.mjs fuzz [--seed S] [--count N] [--shrink] [--out DIR]
//                   [--baseline FILE] [--write-baseline] [--verdicts FILE]
//
// Every command takes --jobs N (default: cores - 1), the number of React
// workers and of analyzer processes running at once, and --assume NAME, an
// assumption the analyzer runs under (ADR-048; `cases`: every case, and only so).
// REACTANT_BIN overrides the analyzer binary (default: target/release/reactant).

import { readFileSync, readdirSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { join, resolve, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { availableParallelism } from "node:os";
import { ReactPool } from "../src/pool.mjs";
import { analyze, claims, reactantBinary, setConcurrency } from "../src/reactant.mjs";
import { judge, VERDICTS, VIOLATIONS } from "../src/judge.mjs";
import { generate } from "../src/gen.mjs";
import { printProgram } from "../src/print.mjs";
import { shrink } from "../src/shrink.mjs";

const here = dirname(fileURLToPath(import.meta.url));

function parseArgs(argv) {
  const opts = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (next === undefined || next.startsWith("--")) opts[key] = true;
      else opts[key] = argv[++i];
    } else opts._.push(a);
  }
  return opts;
}

/** `assume`: the assumptions the analyzer runs under (ADR-048); React does not change. */
async function check(pool, source, assume = []) {
  const [react, report] = await Promise.all([pool.run(source), analyze(source, { assume })]);
  const c = claims(report);
  return { react, claims: c, ...judge(react, c) };
}

function describe(r) {
  const renders = Object.entries(r.react.renders).map(([k, n]) => `${k}×${n}`).join(" ");
  const lines = [
    `react:    ${r.react.verdict}${r.react.via ? ` (${r.react.via})` : ""}${r.react.inLoop?.length ? ` through ${r.react.inLoop.join(", ")}` : ""}  renders: ${renders || "-"}${r.react.warnings.length ? `  warnings: ${r.react.warnings.join(", ")}` : ""}`,
    `reactant: ${r.claims.loops.length ? r.claims.loops.map((l) => `${l.severity} ${l.rule} on ${l.component}:${l.line ?? "?"}`).join("; ") : "no loop claim"}${r.claims.handOvers.length ? `  (+${r.claims.handOvers.length} info hand-over: ${r.claims.handOvers.map((l) => `${l.rule} on ${l.component}:${l.line ?? "?"}`).join("; ")})` : ""}${r.claims.limits.length ? `  (+${r.claims.limits.length} analysis-limit)` : ""}`,
    `verdict:  ${r.verdict}${r.why ? ` — ${r.why}` : ""}`,
  ];
  return lines.join("\n");
}

/** `--jobs N` bounds both the React workers and the analyzer processes. */
function startPool(opts) {
  const jobs = Number(opts.jobs ?? Math.max(1, availableParallelism() - 1));
  setConcurrency(jobs);
  return new ReactPool({ size: jobs });
}

/**
 * A program that changed the realm of its worker (realm.mjs) got that worker replaced: say so, and
 * which program, so that a verdict that ever depends on a neighbour leaves a trail.
 */
function reportRespawns(pool, programs) {
  console.log(`${pool.respawned} workers respawned after a program changed the realm`);
  for (const [name, changed] of programs) {
    if (changed?.length) console.log(`  ${name}: ${changed.slice(0, 3).join(", ")}${changed.length > 3 ? `, +${changed.length - 3} more` : ""}`);
  }
}

function requireBinary() {
  const bin = reactantBinary();
  if (!existsSync(bin)) {
    console.error(`reactant binary not found at ${bin}; build it (cargo build --release) or set REACTANT_BIN`);
    process.exit(2);
  }
}

async function cmdRun(opts) {
  requireBinary();
  const pool = startPool(opts);
  const ran = [];
  const assume = typeof opts.assume === "string" ? [opts.assume] : [];
  for (const f of opts._) {
    const r = await check(pool, readFileSync(f, "utf8"), assume);
    console.log(`── ${f}${assume.length ? ` (assume ${assume.join(", ")})` : ""}\n${describe(r)}\n`);
    ran.push([f, r.react.realmChanged]);
  }
  await pool.close();
  reportRespawns(pool, ran);
}

/**
 * `// oracle: react=loop`, `// oracle: known=fn #168` and
 * `// oracle: assume=<name> fn` headers. The last says what the case's verdict
 * is when the analyzer runs under that assumption (ADR-048): `cases` runs it a second time so,
 * and only there is a violation the expected verdict — the assumption's stated price.
 */
function caseHeader(source) {
  const h = { react: null, known: null, issue: null, assumed: new Map() };
  for (const m of source.matchAll(/^\/\/ oracle: (\w+)=(\S+)(?: (#\d+|[\w-]+))?/gm)) {
    if (m[1] === "react") h.react = m[2];
    if (m[1] === "known") {
      h.known = m[2];
      h.issue = m[3] ?? null;
    }
    if (m[1] === "assume" && m[3]) h.assumed.set(m[2], m[3]);
  }
  return h;
}

/**
 * The problems of one run of a case: `assume` is the assumption it ran under, `null` for the
 * default run. Under an assumption the case states a verdict for, that verdict is the only one
 * accepted; otherwise the default rules hold (a violation fails unless `known`).
 */
function caseProblems(r, h, assume) {
  const problems = [];
  if (!h.react) problems.push("missing `// oracle: react=loop|converges|perpetual` header");
  else if (h.react !== r.react.verdict) problems.push(`React did \`${r.react.verdict}\`, the case expects \`${h.react}\`: the reading of React in docs/semantics.md is wrong here`);
  const stated = assume ? h.assumed.get(assume) : undefined;
  if (stated) {
    if (stated !== r.verdict) problems.push(`under ${assume} the case expects \`${stated}\`, the verdict is ${r.verdict}${r.why ? `: ${r.why}` : ""}`);
    return problems;
  }
  if (VIOLATIONS.has(r.verdict) && h.known !== r.verdict) problems.push(`new ${r.verdict}${assume ? ` under ${assume}` : ""}: ${r.why}`);
  if (h.known && h.known !== r.verdict) problems.push(`marked known=${h.known} ${h.issue ?? ""} but the verdict is now ${r.verdict}: if the issue is fixed, drop the \`known\` line`);
  return problems;
}

async function cmdCases(opts) {
  requireBinary();
  const dir = resolve(opts._[0] ?? join(here, "../cases"));
  const files = readdirSync(dir).filter((f) => /\.(tsx|jsx)$/.test(f)).sort();
  const pool = startPool(opts);
  // `--assume NAME`: every case runs under it, and only so. By default every case runs as it is,
  // and a case that states its verdict under an assumption runs a second time under it.
  const only = typeof opts.assume === "string" ? opts.assume : null;
  let failures = 0;
  let runs = 0;
  const results = await Promise.all(
    files.map(async (f) => {
      const source = readFileSync(join(dir, f), "utf8");
      const h = caseHeader(source);
      const ways = only ? [only] : [null, ...h.assumed.keys()];
      const rs = await Promise.all(ways.map(async (a) => [a, await check(pool, source, a ? [a] : [])]));
      return [f, rs, h];
    }),
  );
  for (const [f, rs, h] of results) {
    for (const [assume, r] of rs) {
      runs += 1;
      const problems = caseProblems(r, h, assume);
      const stated = assume && h.assumed.get(assume);
      const tag = problems.length ? "FAIL" : stated && VIOLATIONS.has(r.verdict) ? "price" : VIOLATIONS.has(r.verdict) ? `known ${h.issue ?? ""}`.trim() : "ok";
      console.log(`${tag.padEnd(10)} ${f.padEnd(40)} react=${r.react.verdict.padEnd(10)} ${r.verdict}${assume ? `  (assume ${assume})` : ""}`);
      for (const p of problems) console.log(`           ${p}`);
      if (problems.length) console.log(describe(r).replace(/^/gm, "           "));
      failures += problems.length ? 1 : 0;
    }
  }
  await pool.close();
  console.log(`\n${files.length} cases, ${failures} failing (${runs} runs${only ? `, all under ${only}` : ""})`);
  reportRespawns(pool, results.flatMap(([f, rs]) => rs.map(([, r]) => [f, r.react.realmChanged])));
  process.exit(failures ? 1 : 0);
}

/** Names are the generator's; canonical text is enough to merge duplicates. */
function signature(source) {
  return source.replace(/\s+/g, " ").trim();
}

async function cmdFuzz(opts) {
  requireBinary();
  const seed = Number(opts.seed ?? 1);
  const count = Number(opts.count ?? 200);
  const pool = startPool(opts);
  const tally = Object.fromEntries(VERDICTS.map((v) => [v, 0]));
  const violations = {};
  const started = Date.now();
  // `--assume NAME`: the analyzer runs under that assumption (ADR-048).
  const assume = typeof opts.assume === "string" ? [opts.assume] : [];

  const ids = Array.from({ length: count }, (_, i) => i);
  const results = new Array(count);
  let done = 0;
  const concurrency = pool.idle.length * 2;
  await Promise.all(
    Array.from({ length: concurrency }, async () => {
      while (ids.length) {
        const i = ids.shift();
        const program = generate(seed, i);
        const source = printProgram(program);
        results[i] = { program, source, ...(await check(pool, source, assume)) };
        if (++done % 50 === 0) process.stderr.write(`  ${done}/${count}\r`);
      }
    }),
  );

  for (const [i, r] of results.entries()) {
    tally[r.verdict] += 1;
    if (VIOLATIONS.has(r.verdict)) violations[i] = r.verdict;
  }
  const secs = ((Date.now() - started) / 1000).toFixed(1);
  console.log(`seed ${seed}, ${count} programs in ${secs}s${assume.length ? `, under ${assume.join(", ")}` : ""}`);
  // `--verdicts FILE`: every program's verdict, by index, to compare two runs program by program.
  if (typeof opts.verdicts === "string") writeFileSync(resolve(opts.verdicts), JSON.stringify(results.map((r) => r.verdict)) + "\n");
  console.log(VERDICTS.map((v) => `${v}=${tally[v]}`).join("  "));
  reportRespawns(pool, results.map((r, i) => [`program #${i}`, r.react.realmChanged]));
  const fns = results.filter((r) => r.verdict === "fn");
  if (fns.length) {
    const share = (p) => fns.filter(p).length;
    console.log(
      `fn: ${share((r) => /\bfrom "ext"/.test(r.source))} through ext, ${share((r) => r.source.includes("// @file"))} multi-file, ` +
        `${share((r) => r.claims.loops.length > 0)} claimed outside the loop, ${share((r) => r.limited)} with an analysis-limit (overlapping)`,
    );
  }

  const outDir = opts.out ? resolve(opts.out) : null;
  if (outDir) mkdirSync(outDir, { recursive: true });
  const reduced = await Promise.all(
    results.map(async (r) => {
      if (!VIOLATIONS.has(r.verdict) || !opts.shrink || r.verdict === "parse-error") return r;
      const target = r.verdict;
      const { program } = await shrink(r.program, async (cand) => (await check(pool, printProgram(cand), assume)).verdict === target);
      const source = printProgram(program);
      return { program, source, ...(await check(pool, source, assume)) };
    }),
  );
  const seen = new Map();
  for (const [i, r] of results.entries()) {
    if (!VIOLATIONS.has(r.verdict)) continue;
    const { source } = reduced[i];
    const final = reduced[i];
    const sig = signature(source);
    if (seen.has(sig)) {
      console.log(`\n#${i} ${r.verdict}: same as #${seen.get(sig)}`);
      continue;
    }
    seen.set(sig, i);
    console.log(`\n#${i} ${final.verdict}${opts.shrink ? " (shrunk)" : ""}\n${describe(final).replace(/^/gm, "  ")}\n${source.replace(/^/gm, "  | ")}`);
    if (outDir) {
      const header = `// oracle: react=${final.react.verdict}\n// found by: oracle fuzz --seed ${seed}, program ${i} (${final.verdict})\n// ${final.why}\n`;
      writeFileSync(join(outDir, `${final.verdict}-s${seed}-${i}.tsx`), header + source);
    }
  }

  const summary = { seed, count, tally, violations };
  if (opts["write-baseline"]) {
    writeFileSync(resolve(opts.baseline ?? join(here, "../fuzz-baseline.json")), JSON.stringify(summary, null, 2) + "\n");
    console.log(`\nbaseline written`);
  } else if (opts.baseline) {
    const base = JSON.parse(readFileSync(resolve(opts.baseline), "utf8"));
    if (base.seed !== seed || base.count !== count) {
      console.error(`baseline is for seed ${base.seed} × ${base.count}, this run is seed ${seed} × ${count}`);
      await pool.close();
      process.exit(2);
    }
    const added = Object.keys(violations).filter((k) => base.violations[k] !== violations[k]);
    const gone = Object.keys(base.violations).filter((k) => violations[k] !== base.violations[k]);
    for (const k of added) console.log(`NEW   program ${k}: ${violations[k]}`);
    for (const k of gone) console.log(`GONE  program ${k}: was ${base.violations[k]}, now ${results[k].verdict}`);
    if (added.length || gone.length) {
      console.log(`\nthe violations differ from ${basename(opts.baseline)}; if the change is intended, rerun with --write-baseline`);
      await pool.close();
      process.exit(1);
    }
    console.log(`\nviolations match ${basename(opts.baseline)}`);
  }
  await pool.close();
}

const opts = parseArgs(process.argv.slice(2));
const cmd = opts._.shift();
const commands = { run: cmdRun, cases: cmdCases, fuzz: cmdFuzz };
if (!commands[cmd]) {
  console.error("usage: oracle.mjs run FILE... | cases [DIR] | fuzz [--seed S] [--count N] [--shrink] [--out DIR] [--baseline FILE] [--write-baseline]");
  process.exit(2);
}
await commands[cmd](opts);
