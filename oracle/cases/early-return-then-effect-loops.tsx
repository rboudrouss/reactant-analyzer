// oracle: react=loop
// #344 (found there). The hooks sit after an early return, so the render's
// exit joins a side that never binds `setN`: the join kept the setter only
// when the binding side came first, and the effect's write was lost here.
// React renders `App` with no props, takes the hooks and loops.
import { useState, useEffect } from "react";

export function App({ c }) {
  if (c) {
    return <div />;
  }
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}
