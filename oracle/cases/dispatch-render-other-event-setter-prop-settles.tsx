// oracle: react=converges
// #407, the negative: the render focuses the button whose `onClick` is the
// setter. A focus fires the focus events, which React DOM never runs
// `onClick` for, so the setter never runs.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setM(1);
  }, []);
  ref.current?.focus();
  return (
    <button ref={ref} onClick={setN}>
      {m}
      {String(!!n)}
    </button>
  );
}
