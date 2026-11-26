// oracle: react=loop
// `handover-info-package-child-effect-loops` with the child in the program: the
// analysis sees the effect that calls the prop, so the loop is a claim at
// Warning (`tp`), not an Info hand-over.
import { useEffect, useState } from "react";

function EffectCalls({ fn }: { fn: () => void }) {
  useEffect(() => {
    fn();
  });
  return null;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <EffectCalls fn={() => setN(n + 1)} />
    </div>
  );
}
