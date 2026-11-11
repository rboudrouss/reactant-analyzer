// oracle: react=loop
// #291, the escaped-closure variant of setter-callback-foreach-effect-loops:
// the closure an unresolved child calls hands the setter to `forEach`.
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
  return <kids.Kid api={{ run: () => [1].forEach(setV) }} />;
}
