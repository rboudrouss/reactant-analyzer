// oracle: react=converges
// #421, the settling neighbour of joined-callback-prop-value-loops: both
// closures the prop holds store the same constant, so the joined row still
// stores one value known in advance and nothing moves the slot away from it.
import { useEffect, useState } from "react";

function C0({ on0, p1 }: { on0: (x: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1);
  }, [p1]);
  return null;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      <C0 on0={() => setS(0)} p1={1} />
      <C0 on0={() => setS(0)} p1={s + 2} />
    </div>
  );
}
