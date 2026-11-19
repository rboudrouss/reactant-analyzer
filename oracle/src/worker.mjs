import { parentPort } from "node:worker_threads";
import { runProgram, reportUncaught } from "./react-run.mjs";

// An uncaught error of a loop (react-run.mjs) is part of the verdict; any other
// one is rethrown and kills the worker, which the pool reports as a crash.
process.on("uncaughtException", (e) => {
  if (!reportUncaught(e)) throw e;
});

parentPort.on("message", async ({ id, source, opts }) => {
  let result;
  try {
    result = await runProgram(source, opts);
  } catch (e) {
    result = { verdict: "crash", via: `runner: ${e?.message ?? e}`, renders: {}, total: 0, warnings: [] };
  }
  parentPort.postMessage({ id, result });
});
