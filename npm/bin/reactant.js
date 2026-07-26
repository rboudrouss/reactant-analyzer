#!/usr/bin/env node
process.exitCode = require("../lib/index.js").main(process.argv.slice(2));
