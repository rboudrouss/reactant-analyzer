// oracle: react=loop
// #373: a class getter read past the inlining budget is not run there. An
// earlier read off another instance ran it, so the read knows what it
// returns, but its body reads `this` and stores it on `globalThis`: the
// receiver is handed over at the read, with the setter it holds, which the
// render then calls through the stored object.
import { useState } from "react";

class Box {
  set: (f: (c: number) => number) => void;
  constructor(set: (f: (c: number) => number) => void) {
    this.set = set;
  }
  get keep() {
    (globalThis as { box?: Box }).box = this;
    return 1;
  }
}

export function App() {
  const [n, setN] = useState(0);
  const quiet = new Box(() => {});
  const w = quiet.keep;
  const o = new Box(setN);
  const a = () => o.keep;
  const b = () => a();
  const c = () => b();
  const v = c();
  (globalThis as { box?: Box }).box?.set((x: number) => x + 1);
  return (
    <div>
      {n}
      {w}
      {v}
    </div>
  );
}
