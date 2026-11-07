// oracle: react=converges
// #254, the negative: the escaped closure writes `null`, the value that keeps
// `if (sel)` false. Its write joins the store and kills nothing, and no
// value of `sel` takes the guarded side (#255).
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [sel, setSel] = useState<object | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => setSel(null));
  }, []);
  useEffect(() => {
    if (sel) setN(n + 1);
  });
  return <div>{n}</div>;
}
