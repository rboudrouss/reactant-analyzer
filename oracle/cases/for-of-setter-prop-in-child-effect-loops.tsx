// oracle: react=loop
// A child iterates the setters its parent hands it: each effect run writes
// the parent's slot, which re-renders the child.
import { useEffect, useState } from "react";

function Child({ setters }: { setters: Array<(f: (c: number) => number) => void> }) {
  useEffect(() => {
    for (const set of setters) set((c) => c + 1);
  });
  return <div />;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Child setters={[setN]} />
    </div>
  );
}
