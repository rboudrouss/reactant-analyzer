// oracle: react=loop
// A parent's closure escapes in a child through a generator, and the child
// declares a setter of the same name: the closure's `setN` is the parent's,
// so the write lands on the parent's slot, which loops.
import { useEffect, useState } from "react";

function* yieldIt(f: (x: number) => void) {
  yield f;
}

function Child({ f }: { f: (x: number) => void }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const g of yieldIt(f)) g(1);
  });
  return <div>{n}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Child f={(x) => setN((c) => c + x)} />
    </div>
  );
}
