// oracle: react=loop
// #325. A closure bound to a name and handed to a package's component, which
// the analysis does not see: React's `Profiler` calls `onRender` after every
// commit, so the write re-renders the component, which commits again.
import { Profiler, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const record = () => setN(n + 1);
  return (
    <Profiler id="p" onRender={record}>
      <div>{n}</div>
    </Profiler>
  );
}
