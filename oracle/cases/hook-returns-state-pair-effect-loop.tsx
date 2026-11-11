// oracle: react=loop
// #283: a hook returns the pair `useState` gives it, and the caller's effect
// writes through the setter it destructures. No name the caller binds is a
// setter syntactically: the write is one the converged values carry.
import { useEffect, useState } from "react";

function useCounter() {
  return useState(0);
}

export function App() {
  const [n, setN] = useCounter();
  useEffect(() => {
    setN(n + 1);
  });
  return <div>{n}</div>;
}
