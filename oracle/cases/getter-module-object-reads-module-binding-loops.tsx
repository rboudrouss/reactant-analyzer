// oracle: react=loop
// oracle: known=fn #337
// #335's getter twin: a module object's getter calls the setter the render
// stored in a module binding. A member read has no anchor in the call
// relation, so the getter's write is placed nowhere.
import { useState } from "react";

let setter = null;
const o = {
  get p() {
    if (setter) setter((c) => c + 1);
    return 1;
  },
};

export function App() {
  const [n, setN] = useState(0);
  setter = setN;
  const v = o.p;
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
