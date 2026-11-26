// oracle: react=loop
// One member of a namespace import, called in the render, in a program that
// writes no function into an object it does not allocate: `runNow` is the
// package's code and calls its argument at once. The hand-over is an Info
// (`disclosed`).
import { useState } from "react";
import * as ext from "ext";

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
