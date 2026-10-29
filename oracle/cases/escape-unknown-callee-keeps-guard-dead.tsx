// oracle: react=converges
// #254, the negative: the escaped closure writes `null`, the value that keeps
// `if (sel)` false. Its write joins the store and kills nothing; the claim
// left here is #255's (both sides of every branch run), tn once it lands.
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
