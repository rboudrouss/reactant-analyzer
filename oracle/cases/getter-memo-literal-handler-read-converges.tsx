// oracle: react=converges
// The getter of a memo's literal read only in a handler: a write when the
// user clicks, not in the render (#337).
import { useMemo, useState } from "react";

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
  return <button onClick={() => console.log(o.p)}>{n}</button>;
}
