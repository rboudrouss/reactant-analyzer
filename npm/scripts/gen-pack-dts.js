#!/usr/bin/env node
// Generate lib/pack.d.ts from schemas/pack.schema.json, the SAME schemars
// output the validator compiles from, so the TS types cannot drift from what
// the core accepts (ADR-023 §5: ship a .d.ts generated from the same types
// as pack.schema.json). Run by build.sh after the schemas are written;
// `--check` verifies the committed file is current (smoke test).
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const ROOT = path.join(__dirname, "..");
const SCHEMA = path.join(ROOT, "schemas", "pack.schema.json");
const OUT = path.join(ROOT, "lib", "pack.d.ts");

function main() {
  const schema = JSON.parse(fs.readFileSync(SCHEMA, "utf8"));
  const text = render(schema);
  if (process.argv.includes("--check")) {
    const disk = fs.existsSync(OUT) ? fs.readFileSync(OUT, "utf8") : "";
    if (disk !== text) {
      process.stderr.write(
        "lib/pack.d.ts is stale, regenerate with `node scripts/gen-pack-dts.js`\n",
      );
      process.exit(1);
    }
    process.stdout.write("lib/pack.d.ts is current\n");
    return;
  }
  fs.writeFileSync(OUT, text);
  process.stdout.write(`wrote ${path.relative(process.cwd(), OUT)}\n`);
}

// ── JSON Schema (2020-12, schemars output) → TypeScript ──────────────────────

/** A schema node → a TS type expression. `defs` resolves `$ref`s by name. */
function tsType(node, indent) {
  if (node.$ref) {
    return sanitizeName(node.$ref.replace("#/$defs/", ""));
  }
  if (node.const !== undefined) return JSON.stringify(node.const);
  if (node.enum) return node.enum.map((v) => JSON.stringify(v)).join(" | ");
  const variants = node.oneOf ?? node.anyOf;
  if (variants) {
    const parts = variants.map((v) => tsType(v, indent));
    // `T | null | null` from schemars' Option encoding: dedupe.
    return [...new Set(parts)].join(" | ");
  }
  const types = Array.isArray(node.type) ? node.type : [node.type];
  const parts = types.map((t) => {
    switch (t) {
      case "string":
        return "string";
      case "integer":
      case "number":
        return "number";
      case "boolean":
        return "boolean";
      case "null":
        return "null";
      case "array":
        return `${wrap(tsType(node.items ?? {}, indent))}[]`;
      case "object":
        return objectType(node, indent);
      default:
        return "unknown";
    }
  });
  return [...new Set(parts)].join(" | ");
}

/** Parenthesize a union before `[]`. */
function wrap(t) {
  return t.includes("|") ? `(${t})` : t;
}

function objectType(node, indent) {
  const props = node.properties ?? {};
  const keys = Object.keys(props);
  if (!keys.length) {
    // schemars maps BTreeMap<String, T> to additionalProperties.
    if (node.additionalProperties && typeof node.additionalProperties === "object") {
      return `{ [key: string]: ${tsType(node.additionalProperties, indent)} }`;
    }
    return "Record<string, unknown>";
  }
  const required = new Set(node.required ?? []);
  const pad = "  ".repeat(indent + 1);
  const lines = keys.sort().map((key) => {
    const prop = props[key];
    const opt = required.has(key) ? "" : "?";
    const doc = docComment(prop.description, pad);
    return `${doc}${pad}${JSON.stringify(key)}${opt}: ${tsType(prop, indent + 1)};`;
  });
  return `{\n${lines.join("\n")}\n${"  ".repeat(indent)}}`;
}

function docComment(description, pad) {
  if (!description) return "";
  const body = description
    .split("\n")
    .map((l) => `${pad} * ${l}`.trimEnd())
    .join("\n");
  return `${pad}/**\n${body}\n${pad} */\n`;
}

/** schemars def names like `PVal_Array_of_string` are already identifiers. */
function sanitizeName(name) {
  return name.replace(/[^A-Za-z0-9_]/g, "_");
}

function render(schema) {
  const header = `// GENERATED from schemas/pack.schema.json by scripts/gen-pack-dts.js. Do not edit.
// The schema and the validator compile from the same Rust types, so these
// TypeScript types cannot drift from what the core accepts.
//
// Author a pack as a JS module and compile it with \`reactant packs build\`:
//
//   /** @type {import("reactant-analyzer/lib/pack").Pack} *​/
//   module.exports = { schemaVersion: 1, name: "team", rules: [ /* … */ ] };
//
// The generated JSON is the committed artifact; the analyzer only ever
// consumes the inert JSON.

`;
  const parts = [header];
  const rootDoc = docComment(schema.description, "");
  parts.push(`${rootDoc}export interface ${schema.title ?? "PackFile"} ${objectType(schema, 0)}\n`);
  for (const [name, def] of Object.entries(schema.$defs ?? {})) {
    const doc = docComment(def.description, "");
    const ts = tsType(def, 0);
    // An interface only for a plain single object; a union (tagged enums,
    // PVal) must be a type alias.
    const plainObject = def.type === "object" && !def.oneOf && !def.anyOf;
    if (plainObject) {
      parts.push(`${doc}export interface ${sanitizeName(name)} ${ts}\n`);
    } else {
      parts.push(`${doc}export type ${sanitizeName(name)} = ${ts};\n`);
    }
  }
  parts.push(`/** The type an authored pack module exports (or returns from a function). */
export type Pack = ${schema.title ?? "PackFile"};\n`);
  return parts.join("\n");
}

main();
