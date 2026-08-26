#!/usr/bin/env node
// `main` is sync for every command except `packs build` (dynamic import);
// Promise.resolve handles both without forking the entry point.
Promise.resolve(require("../lib/index.js").main(process.argv.slice(2))).then(
  (code) => {
    process.exitCode = code;
  },
);
