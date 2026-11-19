// oracle: react=converges
// The neighbour of `rest-param-after-spread-in-render-loops`: one argument,
// so the guarded write never runs.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (...fs: any[]) => {
      if (fs.length > 1) fs[1](n + 1);
    };
    run(setN);
  }, [n]);
  return <div>{n}</div>;
}
