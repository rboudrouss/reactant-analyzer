// oracle: react=loop
// owner-closure-second-parent-loops.tsx with the parents swapped: the loop
// is the first parent's, whose closure the child's later analysis (under
// both inputs, the first parent off the stack) leaves to the run it had.
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
      <Child f={() => setA((c) => c + 1)} />
    </div>
  );
}

function Second() {
  const [b, setB] = useState(0);
  return (
    <div>
      {b}
      <Child f={() => setB(1)} />
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
