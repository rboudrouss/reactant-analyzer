// oracle: react=loop
// A parent's closure forwarded to a grandchild that lets it escape runs in
// the parent, two levels up the analysis stack.
import { useState } from "react";
import { useRun } from "ext";

function Leaf({ f }: { f: () => void }) {
  useRun(f);
  return <div />;
}

function Mid({ f }: { f: () => void }) {
  return <Leaf f={f} />;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Mid f={() => setN((c) => c + 1)} />
    </div>
  );
}
