// oracle: react=converges
// The settling neighbour of owner-closure-escaped-write-arms-parent-guard-loops.tsx:
// the closure the child lets escape writes 0, so `n > 0` never holds.
import { useEffect, useState } from "react";
import { runNow } from "ext";

function Child({ f }: { f: () => void }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(f);
  }, []);
  return <div>{n}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (n > 0) setN(n + 1);
  }, [n]);
  return (
    <div>
      {n}
      <Child f={() => setN(0)} />
    </div>
  );
}
