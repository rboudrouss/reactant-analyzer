// oracle: react=converges
// #330, the precision side: `at` hands the closure back, and nothing calls it.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const q = [h].at(0);
  return <div>{n}{typeof q}</div>;
}
