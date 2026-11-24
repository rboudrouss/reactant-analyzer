// oracle: react=loop
// #325. `Tree` renders itself: the analysis does not run the inner `Tree` with
// what this element hands it (a recursion cutoff), so the closure is handed to
// code it does not run there, at an unknown time. The inner effect calls it
// after every commit with a new object: the outer `Tree` re-renders, so does
// the inner one, and its effect runs again.
import { useEffect, useState } from "react";

export function Tree({ depth, onSeen }: { depth: number; onSeen?: (x: object) => void }) {
  const [s, setS] = useState<object | number>(0);
  useEffect(() => {
    onSeen?.({});
  });
  return (
    <div>
      {typeof s}
      {depth > 0 ? <Tree depth={depth - 1} onSeen={(x: object) => setS(x)} /> : null}
    </div>
  );
}

export function App() {
  return <Tree depth={1} />;
}
