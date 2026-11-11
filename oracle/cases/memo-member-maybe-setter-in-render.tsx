// oracle: react=converges
// #240, the must side: the memo is `[v, setV]` or an array the heap does not
// place, and React takes the second. `t[1]` may be the setter: a possible
// render write, a Warning (fp), never a certain one.
import { useMemo, useState } from "react";

function useSlot(c: boolean | undefined) {
  const [v, setV] = useState(0);
  return useMemo(() => (c ? [v, setV] : Object.freeze([v, (_x: number) => {}])), [c, v]);
}

export function App(props: { c?: boolean }) {
  const t = useSlot(props.c);
  t[1](1);
  return <div>{t[0]}</div>;
}
