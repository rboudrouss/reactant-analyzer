// oracle: react=converges
// #233, the must side: `t[1]` is the setter on one arm and a no-op on the
// other. The converged-value table kept the first block's reading, where
// `t[1]` is surely the setter, and `setter-in-render` certified an Error that
// is false whenever `c` is falsy. A possible write, never a certain one.
import { useState } from "react";

function useThing(c: boolean | undefined) {
  const [v, setV] = useState(0);
  return c ? ([v, setV] as const) : ([v, (_x: number) => {}] as const);
}

export function App(props: { c?: boolean }) {
  const t = useThing(props.c);
  (t[1] as (x: number) => void)(1);
  return <div>{t[0]}</div>;
}
