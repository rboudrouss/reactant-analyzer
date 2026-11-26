// oracle: react=loop
// A reflective writer called through a computed member (`O[k](ext, src)`,
// here `Object.assign`): the key may name any writer, so a computed member
// call whose first argument is rooted at a package import, with a value
// argument, counts as a write into that import (#403). It installs a program
// function on the package object and React loops; no callee is a package's
// any more and the hand-over is a Warning (`tp`). A spread first argument
// (`a[e](...t)`) names no target and stays the premise's.
import { useState } from "react";
import ext from "ext";

const O: any = Object;
const k = "assign";
O[k](ext, { hook: (f: () => void) => f() });

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
