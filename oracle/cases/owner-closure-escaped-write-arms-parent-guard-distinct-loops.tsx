// oracle: react=loop
// owner-closure-escaped-write-arms-parent-guard-loops.tsx without the
// child's same-named setter.
import { useEffect, useState } from "react";
import { runNow } from "ext";

function Child({ f }: { f: () => void }) {
  useEffect(() => {
    runNow(f);
  }, []);
  return <div />;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (n > 0) setN(n + 1);
  }, [n]);
  return (
    <div>
      {n}
      <Child f={() => setN(1)} />
    </div>
  );
}
