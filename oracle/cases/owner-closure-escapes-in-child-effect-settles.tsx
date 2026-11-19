// oracle: react=converges
// The settling neighbour of owner-closure-escapes-in-child-effect-loops.tsx:
// the parent's closure writes a constant, which React bails out of.
import { useEffect, useState } from "react";
import { runNow } from "ext";

function Child({ f }: { f: () => void }) {
  useEffect(() => {
    runNow(f);
  });
  return <div />;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Child f={() => setN(1)} />
    </div>
  );
}
