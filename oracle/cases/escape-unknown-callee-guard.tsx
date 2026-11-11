// oracle: react=loop
// #254: the slot `sel` moves only in a closure handed to an unknown callee.
// The store kept it at `null`; only the solver running both sides of every
// branch kept the guarded write alive (#255 makes `if (sel)` dead under ⊥).
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [sel, setSel] = useState<object | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => setSel({}));
  }, []);
  useEffect(() => {
    if (sel) setN(n + 1);
  });
  return <div>{n}</div>;
}
