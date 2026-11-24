// oracle: react=loop
// ADR-049: a parent's closure that reaches the parent's own props object
// escapes in a child. The props object is the parent's, not the child's, so
// the closure runs in the parent, its owner, and the parent's slot still
// takes the write: the loop through the child's effect is still claimed.
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
      <Child f={() => setN((c) => c + props.step)} />
    </div>
  );
}

export function App() {
  return <Parent step={1} />;
}
