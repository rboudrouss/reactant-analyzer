// oracle: react=loop
// Two parents render the same child, which lets the closure it is handed
// escape. The child is analysed again under both parents' input; the first
// parent's closure has run in its owner already, the second parent's runs
// in the second parent, whose slot loops.
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
      <Child f={() => setB((c) => c + 1)} />
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
