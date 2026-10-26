// oracle: react=loop
// #222 with #204. `d` is `t` on the arm that runs: the write reads `t` as
// well as `s`, and `t` moves without re-running the effect.
import { useEffect, useState } from "react";

export function App() {
  const [c] = useState(false);
  const [s] = useState(false);
  const [t, setT] = useState(false);
  const [b, setB] = useState(false);
  const [y, setY] = useState<boolean | null>(null);
  const d = c ? s : t;
  useEffect(() => {
    setY(d);
  }, [b, s]);
  useEffect(() => {
    setT(!t);
    setB(!b);
  }, [y]);
  return <div>{String(y)}</div>;
}
