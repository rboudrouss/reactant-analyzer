// oracle: react=loop
// #335. The render builds the instance and reads a getter the class
// defines, which calls the setter the constructor kept.
import { useState } from "react";

class K {
  constructor(set) {
    this.set = set;
  }
  get p() {
    this.set((c) => c + 1);
    return 1;
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = new K(setN);
  const v = k.p;
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
