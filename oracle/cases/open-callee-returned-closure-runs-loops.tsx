// oracle: react=loop
// #328. `f` is `make`, which the analysis does not see, or `mk`. The call
// runs `mk`'s body; its value is what that body returns joined with what
// `make` may: `g` holds `mk`'s returned closure, and `g()` runs it. React
// renders `App` with no `make`: the write arms the effect on `n`.
import { useEffect, useState } from "react";

export function App({ make }: { make?: () => () => void }) {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  const f = make ?? mk;
  useEffect(() => {
    const g = f();
    g();
  }, [n]);
  return <div>{n}</div>;
}
