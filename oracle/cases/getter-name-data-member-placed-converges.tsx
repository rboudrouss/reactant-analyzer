// oracle: react=converges
// #334, the precise twin: `p` is a getter's name elsewhere in the program,
// but the memo's literal holds `p` as a plain member, one number the
// effect reads once.
import { useEffect, useMemo, useState } from "react";

const g = {
  get p() {
    return {};
  },
};

export function App() {
  const [n, setN] = useState(0);
  const o = useMemo(() => ({ p: 1 }), []);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{g.p ? n : 0}</div>;
}
