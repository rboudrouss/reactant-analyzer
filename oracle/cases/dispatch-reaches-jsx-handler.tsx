// oracle: react=loop
// oracle: known=fn
// An effect dispatches `click` on the element whose JSX `onClick` writes the
// state: the handler runs inside the effect, every run re-renders. The
// handler is its own entry point, and a dispatch makes its writes of unknown
// timing in the handler's region, never writes of the dispatching effect, so
// the churn graph has no edge for the loop.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.click();
  });
  return (
    <button ref={ref} onClick={() => setN(n + 1)}>
      {n}
    </button>
  );
}
