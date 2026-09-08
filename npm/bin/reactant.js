#!/usr/bin/env node
// The core's glue is ESM and loads through `import()`, so `main` is async.
// The exit code is assigned rather than `process.exit`ed, so buffered stdout
// is flushed before the process leaves.
import { main } from "../lib/cli.js";

process.exitCode = await main(process.argv.slice(2));
