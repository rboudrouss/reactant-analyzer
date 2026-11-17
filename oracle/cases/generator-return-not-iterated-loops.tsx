// oracle: react=loop
// The `[0]` a generator returns is not what a `for…of` over its call
// iterates (#317): what it yields is, and it moves at every run.
import { useEffect, useState } from "react";

function* gen(k: number) {
  yield k + 1;
  return [0];
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const x of gen(n)) setN(x);
  });
  return <div>{n}</div>;
}
