// oracle: react=loop
// #294: a package hook may hand back a new object at every call, so an
// effect on its result re-runs after every render; this one's own write
// re-renders it.
import { useState, useEffect } from "react";
import { useFresh } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const d = useFresh();
  useEffect(() => {
    setN(n + 1);
  }, [d]);
  return <div>{n}</div>;
}
