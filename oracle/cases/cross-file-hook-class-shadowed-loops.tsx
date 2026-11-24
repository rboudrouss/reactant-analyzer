// @file App.tsx
// oracle: react=loop
// #356. `new K(setN)` in a hook of another file constructs that file's `K`,
// whose `run` calls the setter it keeps, not the component file's `K`,
// whose `run` does not: the hook's render writes the state at every render.
import { useX } from "./hook";

export class K {
  set: (f: (c: number) => number) => void;
  constructor(set: (f: (c: number) => number) => void) {
    this.set = set;
  }
  run() {
    return 1;
  }
}

export function App() {
  const v = useX();
  return <div>{v + new K(() => {}).run()}</div>;
}

// @file hook.tsx
import { useMemo, useState } from "react";

class K {
  set: (f: (c: number) => number) => void;
  constructor(set: (f: (c: number) => number) => void) {
    this.set = set;
  }
  run() {
    this.set((c) => c + 1);
    return 1;
  }
}

export function useX() {
  const [n, setN] = useState(0);
  const k = useMemo(() => new K(setN), []);
  return n + k.run();
}
