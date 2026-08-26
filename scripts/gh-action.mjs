#!/usr/bin/env node
// Backend of action.yml: runs the published CLI via npx, turns its JSON
// output (docs/usage.md § JSON schema v2) into GitHub annotations, step
// outputs, and a job-summary line, then exits with the CLI's exit code.
import { spawnSync } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const input = (name, dflt = "") =>
  (process.env[`INPUT_${name.toUpperCase().replace(/-/g, "_")}`] ?? dflt).trim() || dflt;

const paths = input("path", ".").split(/\s+/);
const failOn = input("fail-on", "warning");
const version = input("version", "latest");
const config = input("config");
const extra = input("args") ? input("args").split(/\s+/) : [];

const args = [
  "--yes", `reactant-analyzer@${version}`,
  "check", ...paths, "--format", "json", "--fail-on", failOn,
];
if (config) args.push("--config", config);
args.push(...extra);

const res = spawnSync("npx", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
if (res.stderr) process.stderr.write(res.stderr);
if (res.error || res.status === null) {
  console.log(`::error::failed to run reactant-analyzer: ${res.error?.message ?? "killed"}`);
  process.exit(2);
}

let report;
try {
  report = JSON.parse(res.stdout);
} catch {
  // Exit 2 (usage/IO error) prints no JSON document — surface stderr as-is.
  console.log(`::error::reactant exited ${res.status} without a JSON report (see log above)`);
  process.exit(res.status || 2);
}

// Workflow-command escaping: %, CR, LF in messages; plus : and , in properties.
const escData = (s) => String(s).replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A");
const escProp = (s) => escData(s).replace(/:/g, "%3A").replace(/,/g, "%2C");
const level = { error: "error", warning: "warning", info: "notice" };

for (const pe of report.parse_errors ?? []) {
  console.log(`::warning file=${escProp(pe.file)},title=parse error::${escData(pe.message)} — file skipped, findings inside it are not proven absent`);
}

for (const d of report.diagnostics ?? []) {
  const props = [`title=${escProp(d.rule)}`];
  if (d.file) props.push(`file=${escProp(d.file)}`);
  if (d.line != null) {
    props.push(`line=${d.line}`);
    props.push(`col=${(d.col ?? 0) + 1}`); // JSON col is 0-indexed, annotations are 1-indexed
  }
  const msg = [
    `[${d.component}] ${d.message}`,
    ...(d.notes ?? []).map((n) => `→ ${n.message}`),
  ].join("\n");
  console.log(`::${level[d.severity] ?? "notice"} ${props.join(",")}::${escData(msg)}`);
}

const s = report.summary ?? {};
const jsonPath = join(process.env.RUNNER_TEMP ?? ".", "reactant-report.json");
writeFileSync(jsonPath, res.stdout);

if (process.env.GITHUB_OUTPUT) {
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `errors=${s.errors ?? 0}\nwarnings=${s.warnings ?? 0}\ninfos=${s.infos ?? 0}\n` +
      `exit-code=${res.status}\njson=${jsonPath}\n`,
  );
}
if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(
    process.env.GITHUB_STEP_SUMMARY,
    `### reactant\n\n${s.errors ?? 0} error(s), ${s.warnings ?? 0} warning(s) across ` +
      `${report.files_analyzed ?? 0} file(s), ${s.components_analyzed ?? 0} component(s) analyzed ` +
      `(fail-on: ${failOn}).\n`,
  );
}

console.log(
  `reactant: ${s.errors ?? 0} error(s), ${s.warnings ?? 0} warning(s), ` +
    `${s.infos ?? 0} info(s) — exit ${res.status}`,
);
process.exit(res.status);
