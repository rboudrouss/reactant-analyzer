// oracle: react=loop
// A generator's call is an iterator over what it yields, not the value its
// body returns (#317): the setter it yields is called by the loop.
import { useEffect, useState } from "react";

function* gen(s: (v: number) => void) {
  yield s;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const f of gen(setN)) f(n + 1);
  });
  return <div>{n}</div>;
}
