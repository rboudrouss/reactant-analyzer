import { useEffect, useState } from "react";

// A finding the run *does* see, so the caveat has to coexist with real output:
// a blind spot makes the counts a lower bound, it does not replace them.
export function Broken() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return <div>{n}</div>;
}
