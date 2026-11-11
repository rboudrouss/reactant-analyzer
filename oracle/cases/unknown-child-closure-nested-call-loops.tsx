// oracle: react=loop
// #289, the nested-call variant of unknown-child-closure-local-alias-loops:
// the closure an unresolved child receives writes through a render function
// it calls.
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
  const bump = () => setV(1);
  useEffect(() => {
    if (v > 0) setV(v + 1);
  }, [v]);
  return <kids.Kid api={{ run: () => bump() }} />;
}
