// oracle: react=converges
// #249: the fresh write would move `s` and re-run the memo, but the pass
// stores its last write, `s` itself: the slot holds and the memo stops. An
// exact dep certifies a loop only when every render-phase write moves it.
import { useMemo, useState } from "react";

export function App() {
  const [s, setS] = useState({});
  const m = useMemo(() => {
    setS({});
    setS(s);
    return Object.keys(s).length;
  }, [s]);
  return <div>{m}</div>;
}
