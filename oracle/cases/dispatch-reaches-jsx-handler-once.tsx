// oracle: react=converges
// #277, the negative: the effect is mount-only and the handler it fires
// writes a constant, so the state settles after one write.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.click();
  }, []);
  return (
    <button ref={ref} onClick={() => setN(1)}>
      {n}
    </button>
  );
}
