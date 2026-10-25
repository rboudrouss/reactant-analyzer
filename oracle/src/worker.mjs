import { parentPort } from "node:worker_threads";
import { runProgram } from "./react-run.mjs";

parentPort.on("message", async ({ id, source, opts }) => {
  let result;
  try {
    result = await runProgram(source, opts);
  } catch (e) {
    result = { verdict: "crash", via: `runner: ${e?.message ?? e}`, renders: {}, total: 0, warnings: [] };
  }
  parentPort.postMessage({ id, result });
});
