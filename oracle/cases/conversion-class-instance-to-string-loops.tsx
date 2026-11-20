// oracle: react=loop
// #360: a template literal converts an instance of a class of the program,
// which calls the class's `toString` with the instance as `this` (#335,
// #338): it calls the setter the instance holds, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;

class K {
  set: Set;
  constructor(set: Set) {
    this.set = set;
  }
  toString() {
    this.set((c) => c + 1);
    return "k";
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = new K(setN);
  const s = `${k}`;
  return <div>{n}{s}</div>;
}
