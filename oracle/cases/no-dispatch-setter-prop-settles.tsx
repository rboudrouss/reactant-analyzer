// oracle: react=converges
// #407, the negative: the setter handed as `onClick`, in a program that
// dispatches no event: only a user's click runs it.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setM(1);
  }, []);
  return (
    <button ref={ref} onClick={setN}>
      {m}
      {String(!!n)}
    </button>
  );
}
