// oracle: react=loop
// #328. `f` is a closure of the component or the setter `setA`, which the
// heap does not place: the call writes `a` and runs `mk`'s body, and its
// value holds the closure `mk` returns, which the next call runs. React
// renders `App` with `c` true: the returned writer arms the effect on `n`.
import { useEffect, useState } from "react";

export function App({ c = true }: { c?: boolean }) {
  const [a, setA] = useState(0);
  const [n, setN] = useState(0);
  const mk = (_v: number) => () => setN(n + 1);
  const f: (v: number) => unknown = c ? mk : setA;
  useEffect(() => {
    const r = f(1);
    if (typeof r === "function") r();
  }, [n]);
  return (
    <div>
      {a}
      {n}
    </div>
  );
}
