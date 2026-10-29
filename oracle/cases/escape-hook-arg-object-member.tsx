// oracle: react=loop
// #254: the closure is a member of an object handed to a hook the analyzer
// cannot inline (`useTable({ onClick })`): every closure the object reaches
// may be called.
import { useEffect, useState } from "react";
import { useRun } from "ext";

export function App() {
  const [k, setK] = useState(0);
  const [n, setN] = useState(0);
  const options = { label: "row", onRow: () => setK(1) };
  useRun(options);
  useEffect(() => {
    setN(n + k);
  });
  return <div>{n}</div>;
}
