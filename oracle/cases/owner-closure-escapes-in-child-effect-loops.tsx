// oracle: react=loop
// A parent's closure that a child hands to code the analysis does not see
// runs in the parent, its owner, not in the child: the parent's slot still
// takes the write and the loop through the child's effect is still claimed.
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
      <Child f={() => setN((c) => c + 1)} />
    </div>
  );
}
