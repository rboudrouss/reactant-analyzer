// oracle: react=converges
// #313. The effect clicks the button through a computed method name, which
// no syntactic detector reads as a dispatch: the click handler restores the
// committed value in the step's own batch and React bails out. A handler's
// write is in the batch whenever anything may dispatch its event.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setN(n + 1);
    const m = "click";
    (ref.current as any)?.[m]();
  }, [n]);
  return (
    <button ref={ref} onClick={() => setN(n)}>
      {n}
    </button>
  );
}
