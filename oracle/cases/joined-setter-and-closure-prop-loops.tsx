// oracle: react=loop
// #421: one prop holds the parent's setter at one element and a closure at the
// other. `on0(1)` writes `1` through the setter, and through the closure a
// step of the slot the second element's `p1` reads: a loop. The row of the
// call joins the setter's write with the closure's, so it is no longer the
// single value `1` the setter alone would store.
import { useEffect, useState } from "react";

function C0({ on0, p1 }: { on0: (x: number) => void; p1: number }) {
  useEffect(() => {
    on0(1);
  }, [p1]);
  return null;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      <C0 on0={setS} p1={0} />
      <C0 on0={(x) => setS(s + x)} p1={s} />
    </div>
  );
}
