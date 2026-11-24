// oracle: react=loop
// #383. The curried shape: called with no argument, the factory hands back a
// function of the program by name, `counter`, which the component then calls
// with its own argument. The hook's body is `counter`'s (or the closure
// `wrap` returns, chosen once per run), and `counter`'s effect loops.
import { useState, useEffect } from "react";

function counter(step) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + step);
  }, [n]);
  return n;
}

function wrap(step) {
  return () => step;
}

const create = (step) => (step ? wrap(step) : counter);

const useX = create();

export function App() {
  const n = useX(1);
  return <div>{n}</div>;
}
