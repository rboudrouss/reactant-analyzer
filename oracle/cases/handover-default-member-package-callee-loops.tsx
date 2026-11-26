// oracle: react=loop
// A member of a default import, called in the render, in a program that
// writes no function into an object it does not allocate (a primitive on the
// global object, a function on its own literal): the package's code, an Info
// (`disclosed`).
import { useState } from "react";
import ext from "ext";

(globalThis as any).flag = 1;

export function App() {
  const [s, setS] = useState(0);
  const box = { f: () => 0 };
  box.f = () => 1;
  ext.runNow(() => setS(s + box.f()));
  return <div>{s}</div>;
}
