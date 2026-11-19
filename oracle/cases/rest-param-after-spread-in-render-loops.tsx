// oracle: react=loop
// #324: a spread hands the rest its entries, at positions nothing fixes, and
// leaves its length unknown: the length guard does not rule the write out.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  const xs = [setM, setN];
  const run = (...fs: any[]) => {
    if (fs.length > 1) fs[1](n + 1);
  };
  run(...xs);
  return <div>{n}{m}</div>;
}
