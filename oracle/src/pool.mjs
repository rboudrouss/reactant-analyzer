// A pool of worker threads running `runProgram`, one program at a time each.
// A worker that exceeds the time limit is killed and replaced: a program the
// budgets did not stop (a synchronous `while (true)`) costs one worker, not the
// run. So is a worker whose realm a program changed (a getter on
// `Object.prototype`): the next program must not run in it.

import { Worker } from "node:worker_threads";
import { availableParallelism } from "node:os";

const WORKER = new URL("./worker.mjs", import.meta.url);

export class ReactPool {
  constructor({ size = Math.max(1, Math.min(8, availableParallelism() - 1)), timeoutMs = 20_000 } = {}) {
    this.timeoutMs = timeoutMs;
    this.queue = [];
    this.idle = [];
    this.seq = 0;
    /** Workers replaced because a program changed their realm (realm.mjs), not for a timeout or a crash. */
    this.respawned = 0;
    this.workers = new Set();
    for (let i = 0; i < size; i++) this.idle.push(this.spawn());
  }

  spawn() {
    const w = new Worker(WORKER);
    w.unref();
    this.workers.add(w);
    return w;
  }

  run(source, opts = {}) {
    return new Promise((resolve) => {
      this.queue.push({ source, opts, resolve });
      this.pump();
    });
  }

  pump() {
    while (this.idle.length && this.queue.length) {
      const w = this.idle.pop();
      const job = this.queue.shift();
      const id = ++this.seq;
      const done = (result, worker) => {
        clearTimeout(timer);
        w.off("message", onMessage);
        w.off("error", onError);
        this.idle.push(worker);
        job.resolve(result);
        this.pump();
      };
      const replace = () => {
        this.workers.delete(w);
        w.terminate();
        return this.spawn();
      };
      // The result of a program that changed the realm is still its own verdict (it ran in
      // the pristine realm); only the worker is not fit for the next one.
      const onMessage = (m) => {
        if (m.id !== id) return;
        if (!m.realmChanged?.length) return done(m.result, w);
        this.respawned += 1;
        done({ ...m.result, realmChanged: m.realmChanged }, replace());
      };
      const onError = (e) => done({ verdict: "crash", via: `worker: ${e.message}`, renders: {}, total: 0, warnings: [] }, replace());
      const timer = setTimeout(() => done({ verdict: "timeout", via: `${this.timeoutMs} ms`, renders: {}, total: 0, warnings: [] }, replace()), this.timeoutMs);
      w.on("message", onMessage);
      w.on("error", onError);
      w.postMessage({ id, source: job.source, opts: job.opts });
    }
  }

  async close() {
    await Promise.all([...this.workers].map((w) => w.terminate()));
  }
}
