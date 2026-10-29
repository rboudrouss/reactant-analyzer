// oracle: react=converges
// #228. The effect mutates the object `ref.current` holds, in place and
// through `Object.assign`: the object stays the same, so the identity
// `[ref.current]` compares does not move and the effect runs once.
import { useEffect, useRef, useState } from "react";

export function App() {
  const ref = useRef({ n: 0 });
  const [t, setT] = useState(false);
  useEffect(() => {
    Object.assign(ref.current, { n: ref.current.n + 1 });
    ref.current.n = 2;
    setT(!t);
  }, [ref.current]);
  return <div>{String(t)}</div>;
}
