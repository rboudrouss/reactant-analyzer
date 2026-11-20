// oracle: react=loop
// #373: a class getter read past the inlining budget is not run there but runs
// later with `this` unknown. An earlier read off another instance ran it, so
// the read knows what it returns, but its body reads its receiver: the
// receiver is handed over at the read, with the setter it holds, as a
// method's is (#340).
import { useState } from "react";

class Box {
  set: (f: (c: number) => number) => void;
  constructor(set: (f: (c: number) => number) => void) {
    this.set = set;
  }
  get run() {
    this.set((c: number) => c + 1);
    return 1;
  }
}

export function App() {
  const [n, setN] = useState(0);
  const quiet = new Box(() => {});
  const w = quiet.run;
  const o = new Box(setN);
  const a = () => o.run;
  const b = () => a();
  const c = () => b();
  const v = c();
  return (
    <div>
      {n}
      {w}
      {v}
    </div>
  );
}
