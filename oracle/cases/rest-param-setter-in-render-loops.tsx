// oracle: react=loop
// #324: a rest parameter holds the call's arguments from its position on, so
// `fs[0]` is the `setN` the call passes: a render-phase write, placed.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (...fs: any[]) => fs[0](n + 1);
  run(setN);
  return <div>{n}</div>;
}
