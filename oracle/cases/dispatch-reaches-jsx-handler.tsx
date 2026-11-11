// oracle: react=loop
// An effect dispatches `click` on the element whose JSX `onClick` writes the
// state: the handler runs inside the effect, every run re-renders. The
// dispatching call runs the component's `click` handlers where it is made
// (#277), so their write is a possible write of the effect.
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
