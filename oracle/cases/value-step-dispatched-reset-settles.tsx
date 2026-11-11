// oracle: react=converges
// #280: the effect clicks the reset itself, in the step's own batch: no click handler waits for a user.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setN(n + 1);
    r.current?.click();
  }, [n]);
  return (
    <button ref={r} onClick={() => setN(0)}>
      {n}
    </button>
  );
}
