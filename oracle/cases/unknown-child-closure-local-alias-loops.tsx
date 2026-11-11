// oracle: react=loop
// #289: `kids.Kid` is no component the analysis resolves, and the closure it
// receives writes through `s`, a name its own body binds to the setter. The
// escape walk resolved the callees it read without running the body, so `s`
// was no setter, `v` stayed 0, and the loop read as `verified`.
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
  useEffect(() => {
    if (v > 0) setV(v + 1);
  }, [v]);
  return (
    <kids.Kid
      api={{
        run: () => {
          const s = setV;
          s(1);
        },
      }}
    />
  );
}
