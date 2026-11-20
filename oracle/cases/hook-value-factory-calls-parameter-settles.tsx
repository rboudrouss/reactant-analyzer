// oracle: react=converges
// #383. The factory hands back a call of its parameter: what that returns is
// not a function the analysis follows, so the hook stays one it does not
// see. Here the closure the argument builds settles.
import { useState, useEffect } from "react";

function make(build) {
  return build();
}

const useX = make(() => () => {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(1);
  }, [n]);
  return n;
});

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
