// oracle: react=loop
// #324: an inlined utility's rest parameter is the call's own arguments.
import { useState } from "react";

function second(v: number, ...fs: any[]) {
  fs[1](v + 1);
}

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  second(m, setN, setM);
  return <div>{n}{m}</div>;
}
