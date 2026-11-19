// oracle: react=converges
// #313. `C` is mounted through `Comp`, a value the element tree cannot place,
// under a guard its own effect moves: after three ticks the parent renders
// nothing and the child's every-render effect stops. A child no placed
// element mounts is not a root that stays mounted.
import { useEffect, useState } from "react";

function C({ onTick }: { onTick: () => void }) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
    onTick();
  });
  return <div>{Object.keys(s).length}</div>;
}

function D() {
  return <span />;
}

export function App() {
  const [n, setN] = useState(0);
  const Comp = n >= 0 ? C : D;
  return n < 3 ? <Comp onTick={() => setN((x) => x + 1)} /> : null;
}
