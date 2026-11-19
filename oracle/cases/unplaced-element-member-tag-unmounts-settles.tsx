// oracle: react=converges
// #313. `C` is held by a module object and mounted as `<parts.C/>` under a
// guard its effect moves: the child unmounts after three ticks.
import { useEffect, useState } from "react";

function C({ onTick }: { onTick: () => void }) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
    onTick();
  });
  return <div>{Object.keys(s).length}</div>;
}

const parts = { C };

export function App() {
  const [n, setN] = useState(0);
  return n < 3 ? <parts.C onTick={() => setN((x) => x + 1)} /> : null;
}
