// oracle: react=loop
// #328. `f` is one of two closures of the component, each returning a
// writer: the call's value holds both returned closures, so `g()` may write
// `a` or `n`. React renders `App` with no `c`: `mk2`'s writer arms the
// effect on `n`.
import { useEffect, useState } from "react";

export function App({ c }: { c?: boolean }) {
  const [a, setA] = useState(0);
  const [n, setN] = useState(0);
  const mk1 = () => () => setA(a + 1);
  const mk2 = () => () => setN(n + 1);
  const f = c ? mk1 : mk2;
  useEffect(() => {
    const g = f();
    g();
  }, [n]);
  return (
    <div>
      {a}
      {n}
    </div>
  );
}
