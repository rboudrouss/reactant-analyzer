// oracle: react=loop
// #246: a call-returned setter called in an arrow nested in a memo body, the
// arrow the memo returns, run by a no-deps effect. The temp lives in the
// arrow, under a memo body that keeps no per-block envs either.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  const run = useMemo(() => () => get()(v + 1), [v]);
  useEffect(() => {
    run();
  });
  return <div>{v}</div>;
}
