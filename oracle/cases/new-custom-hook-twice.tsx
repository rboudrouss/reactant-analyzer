// oracle: react=loop
// #185: a custom hook called twice in one component. The recursion guard of
// the hook expansion remembered every name it had expanded, so the second
// call — a sibling, not a recursion — stayed opaque, and its effect, the one
// whose dep is fresh every render, was never analysed.
import { useEffect, useState } from "react";

function useTick(dep: unknown) {
  const [c, setC] = useState(0);
  useEffect(() => {
    setC(c + 1);
  }, [dep]);
  return c;
}

export function App() {
  const a = useTick(1);
  const b = useTick({ fresh: true });
  return (
    <div>
      {a}
      {b}
    </div>
  );
}
