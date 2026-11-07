// oracle: react=loop
// #202 (binding half). `s` is bound to `setA` on one arm and `setB` on the
// other: the join keeps both, so the write of `b` under `[b]` is seen. React
// takes the falsy arm: `s` is `setB`, the loop.
import { useEffect, useState } from "react";

export function App(props: { c?: boolean }) {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  const s = props.c ? setA : setB;
  useEffect(() => {
    s(b + 1);
  }, [b]);
  return (
    <div>
      {a}
      {b}
    </div>
  );
}
