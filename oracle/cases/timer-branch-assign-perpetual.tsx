// oracle: react=perpetual
// #214, the timer form: the callback's branch sets the step it adds, so every
// tick moves `n` and re-arms the timer. Each render is paid for by a timer:
// perpetual, which the rules do not claim (docs/semantics.md §5).
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setTimeout(() => {
      let step = 0;
      if (n >= 0) {
        step = 1;
      }
      setN(n + step);
    });
  }, [n]);
  return <div>{n}</div>;
}
