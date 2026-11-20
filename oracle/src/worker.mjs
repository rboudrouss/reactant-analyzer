import { parentPort } from "node:worker_threads";
import { runProgram, reportUncaught, preloadLibraries } from "./react-run.mjs";
import { snapshot, changes } from "./realm.mjs";

// An uncaught error of a loop (react-run.mjs) is part of the verdict; any other
// one is rethrown and kills the worker, which the pool reports as a crash.
process.on("uncaughtException", (e) => {
  if (!reportUncaught(e)) throw e;
});

// A worker runs many programs in one realm, so a program that changes the
// realm (a getter on `Object.prototype`, a `Proxy` as a prototype) would leak
// into the next one. After every program the worker compares the realm with
// this snapshot and, if it differs, says so: its result is still the verdict of
// a program that ran in the pristine realm, and the pool replaces the worker
// before handing it another one (realm.mjs).
//
// The snapshot is taken once the runner has done all its own first-use setup:
// every package loaded, and one empty program rendered (React's first render
// puts a listener marker on `document`). That is the realm every program starts
// in, whichever one a worker happens to run first.
preloadLibraries();
await runProgram("export function App() { return null; }");
const pristine = snapshot();

parentPort.on("message", async ({ id, source, opts }) => {
  let result;
  try {
    result = await runProgram(source, opts);
  } catch (e) {
    result = { verdict: "crash", via: `runner: ${e?.message ?? e}`, renders: {}, total: 0, warnings: [] };
  }
  const realmChanged = changes(pristine);
  parentPort.postMessage({ id, result, realmChanged });
});
