// oracle: react=loop
// #64. `memo(forwardRef(render))`: React calls `render(props, ref)`, `ref` the
// element's own. The parent hands a new callback ref at every render, so the
// memo re-renders, and the child's effect calls it: the parent's state grows
// after every commit.
import { forwardRef, memo, useEffect, useState } from "react";

const Probe = memo(
  forwardRef<number, {}>(function Probe(props, ref) {
    useEffect(() => {
      if (typeof ref === "function") ref(1);
    });
    return null;
  }),
);
Probe.displayName = "Probe";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Probe ref={() => setN((c) => c + 1)} />
    </div>
  );
}
