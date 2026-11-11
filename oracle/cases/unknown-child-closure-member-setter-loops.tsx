// oracle: react=loop
// #286: `kids.Kid` is no component the analysis resolves, so the closure it
// receives may run at any time. It writes through `t[1]`, the setter the
// tuple holds; the escape resolved only names and `obj.field` callees, so `v`
// stayed 0, the effect's guard stayed false, and the loop read as `verified`.
import { useEffect, useState } from "react";

function Kid({ api }: { api: { run: () => void } }) {
  useEffect(() => {
    api.run();
  }, []);
  return null;
}
const kids = { Kid };

export function App() {
  const [v, setV] = useState(0);
  const t = [v, setV] as const;
  useEffect(() => {
    if (v > 0) setV(v + 1);
  }, [v]);
  return <kids.Kid api={{ run: () => t[1](1) }} />;
}
