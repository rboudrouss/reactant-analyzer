// oracle: react=loop
// #233: a hook returning a ternary of literals whose positions differ. The
// caller's `b` is `v` or `setV`, a value that may be a setter or a number,
// and a call of it wrote nothing: only a value that could be nothing but a
// setter named the setters a call writes.
import { useEffect, useState } from "react";

function useFlip(flip: boolean | undefined) {
  const [v, setV] = useState(0);
  return flip ? ([setV, v] as const) : ([v, setV] as const);
}

export function App(props: { flip?: boolean }) {
  const [a, b] = useFlip(props.flip);
  useEffect(() => {
    (b as (f: (n: number) => number) => void)((n) => n + 1);
  });
  return <div>{String(a)}</div>;
}
