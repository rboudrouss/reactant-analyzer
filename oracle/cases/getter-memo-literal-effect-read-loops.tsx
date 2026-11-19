// oracle: react=loop
// The getter of a memo's literal read in an effect with no deps: the read is
// a call of it in the effect, which re-runs after every render (#337).
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = useMemo(
    () => ({
      get p() {
        setN((c) => c + 1);
        return 1;
      },
    }),
    [],
  );
  useEffect(() => {
    const v = o.p;
    if (v < 0) console.log(v);
  });
  return <div>{n}</div>;
}
