// oracle: react=converges
// ADR-049: the closure of `owner-closure-reaches-parent-props-loops`, which
// reaches the parent's props object and runs in the parent, writes the same
// value at every run: the parent bails out and the program settles.
import { useEffect, useState } from "react";
import { runNow } from "ext";

function Child({ f }: { f: () => void }) {
  useEffect(() => {
    runNow(f);
  });
  return <div />;
}

function Parent(props: { step: number }) {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Child f={() => setN(props.step)} />
    </div>
  );
}

export function App() {
  return <Parent step={1} />;
}
