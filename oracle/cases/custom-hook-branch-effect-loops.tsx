// oracle: react=loop
// #344 (found there). A custom hook whose hooks sit on one side of its own
// condition: inlined, the side that does not bind `setN` meets the one that
// does, and the join lost the setter when the binding side came second.
// React renders `App` with no props, takes the hooks and loops.
import { useState, useEffect } from "react";

function useX(skip) {
  if (!skip) {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + 1);
    }, [n]);
    return n;
  }
  return 0;
}

export function App({ skip }) {
  const n = useX(skip);
  return <div>{n}</div>;
}
