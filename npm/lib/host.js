// Host-side transport (ADR-022 §6): find the root, read the raw config,
// resolve packs (require.resolve + the "reactant" package.json field), and
// build the superset file map. NO analyzer semantics live here — discovery,
// project detection, tsconfig chains and every validation run inside the
// wasm core, which re-validates everything it receives.
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

function isDir(p) {
  try {
    return fs.statSync(p).isDirectory();
  } catch {
    return false;
  }
}

// Mirrors the native project-root pick: first directory argument, else ".".
function projectRoot(paths) {
  return paths.map(String).find(isDir) ?? ".";
}

function readConfigText(configPath, root, configFileName) {
  if (configPath) {
    // Explicit --config must exist (usage error handled by the caller).
    return { text: fs.readFileSync(configPath, "utf8"), dir: path.dirname(configPath) };
  }
  const discovered = path.join(root, configFileName);
  if (fs.existsSync(discovered)) {
    return { text: fs.readFileSync(discovered, "utf8"), dir: root };
  }
  return { text: null, dir: root };
}

// Resolve pack specs to their JSON bytes. Relative/absolute paths resolve
// against the config file's directory; npm names via require.resolve of the
// package.json, whose "reactant" field points at the pack file (fallback:
// <pkg>/pack.json).
function resolvePacks(specs, configDir) {
  return specs.map((spec) => {
    let packPath;
    if (spec.startsWith(".") || path.isAbsolute(spec) || spec.endsWith(".json")) {
      packPath = path.resolve(configDir, spec);
    } else {
      const req = createRequire(path.join(path.resolve(configDir), "package.json"));
      let pkgPath;
      try {
        pkgPath = req.resolve(`${spec}/package.json`);
      } catch {
        throw new Error(`pack \`${spec}\`: not installed (require.resolve failed)`);
      }
      const pkg = JSON.parse(fs.readFileSync(pkgPath, "utf8"));
      const rel = pkg.reactant ?? "pack.json";
      packPath = path.join(path.dirname(pkgPath), rel);
    }
    return { name: spec, json: fs.readFileSync(packPath, "utf8") };
  });
}

// Superset walk: every source-extension file plus .gitignore, tsconfig*.json
// and the build-tool configs (vite.config.*, next.config.*) encountered — the
// engine's own discovery re-filters over the map (d.ts/test/spec exclusions
// included) and detects the project kind from those markers.
//
// The host prunes only what no policy would ever read (hostConstants'
// prunedDirs): since #137 the engine's exclusions depend on the tree's own
// .gitignore, and a host that pre-applied a name list would hide files the
// engine wanted — a superset walk that is not a superset. Reading a bit more
// than the engine walks is the price.
function buildFileMap(paths, constants) {
  const files = {};
  const wanted = (name) =>
    constants.sourceExtensions.some((ext) => name.endsWith(`.${ext}`)) ||
    name === ".gitignore" ||
    // Not analyzed: it is one of the two markers that bound the upward
    // `.gitignore` search, and `.git` never reaches the map.
    name === "package.json" ||
    /^tsconfig[^/\\]*\.json$/.test(name) ||
    /^vite\.config\.(ts|js|mjs|mts)$/.test(name) ||
    /^next\.config\.(ts|js|mjs|cjs|mts)$/.test(name);

  const addFile = (p) => {
    try {
      files[toPosix(p)] = fs.readFileSync(p, "utf8");
    } catch {
      // Unreadable file: absent from the map (documented v1 divergence —
      // native records an io parse_error for explicitly-passed files).
    }
  };
  const walk = (dir) => {
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) {
        if (!constants.prunedDirs.includes(e.name)) walk(p);
      } else if (e.isFile() && wanted(e.name)) {
        addFile(p);
      }
    }
  };

  for (const input of paths.length ? paths : ["."]) {
    if (isDir(input)) walk(input);
    else if (fs.existsSync(input)) addFile(input);
    // Nonexistent inputs stay out of the map: the engine emits the same
    // "no such file or directory" usage error as the native CLI.
  }
  return files;
}

// Map keys are cwd-relative POSIX paths: byte-equal display with the native
// CLI run from the same cwd, and no Windows drive-letter semantics in wasm.
function toPosix(p) {
  return p.split(path.sep).join("/");
}

module.exports = { projectRoot, readConfigText, resolvePacks, buildFileMap, isDir };
