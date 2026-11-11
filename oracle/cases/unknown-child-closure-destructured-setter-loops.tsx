// oracle: react=loop
// #289, the destructuring variant of unknown-child-closure-local-alias-loops:
// the closure an unresolved child receives reads the setter out of a tuple.
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
  return (
    <kids.Kid
      api={{
        run: () => {
          const [, s] = t;
          s(1);
        },
      }}
    />
  );
}
