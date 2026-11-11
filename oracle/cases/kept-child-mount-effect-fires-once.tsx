// oracle: react=converges
// #162 (2), the precision side: `Child` is mounted on every path through
// `Mid`, which is itself mounted on every path, so it stays mounted (#213) and
// its `[]` effect clears the slot once; the parent's refill then settles.
import { useEffect, useState } from "react";

function Child({ onReady }: { onReady: (v: boolean) => void }) {
  useEffect(() => {
    onReady(false);
  }, []);
  return null;
}

function Mid({ onReady }: { onReady: (v: boolean) => void }) {
  return <Child onReady={onReady} />;
}

export function App() {
  const [s, setS] = useState(true);
  useEffect(() => {
    if (!s) setS(true);
  }, [s]);
  return <Mid onReady={setS} />;
}
