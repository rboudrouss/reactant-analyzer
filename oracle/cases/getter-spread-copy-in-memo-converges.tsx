// oracle: react=converges
// #321. A spread reads the getter once, inside a memo that never recomputes:
// the copy holds one object as a plain member.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const c = useMemo(() => {
    const o = {
      get p() {
        return {};
      },
    };
    return { ...o };
  }, []);
  useEffect(() => {
    setN((x) => x + 1);
  }, [c.p]);
  return <div>{n}</div>;
}
