// oracle: react=loop
// oracle: known=fn #19
// Found by `oracle fuzz --seed 1` (programs 499, 835, shrunk). `runNow` comes
// from a package the analyzer has no source for and calls its callback at once:
// the effect is `setS(s + 2)` on `[s]`. ADR-009's `Unknown → skip` does not
// descend into the callback, and nothing says so: no analysis-limit, and
// `infinite-loop` is reported verified. `defer` (a microtask) is missed the same way.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    runNow(() => setS(s + 2));
  }, [s]);
  return <div />;
}
