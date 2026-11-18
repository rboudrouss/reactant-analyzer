// oracle: react=converges
// The settling neighbour of owner-closure-second-parent-loops.tsx: both
// parents' closures write a constant.
import { useEffect, useState } from "react";
import { runNow } from "ext";

function Child({ f }: { f: () => void }) {
  useEffect(() => {
    runNow(f);
  });
  return <div />;
}

function First() {
  const [a, setA] = useState(0);
  return (
    <div>
      {a}
      <Child f={() => setA(1)} />
    </div>
  );
}

function Second() {
  const [b, setB] = useState(0);
  return (
    <div>
      {b}
      <Child f={() => setB(2)} />
    </div>
  );
}

export function App() {
  return (
    <div>
      <First />
      <Second />
    </div>
  );
}
