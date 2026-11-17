// oracle: react=loop
// `Array.from` of a generator's call holds what it yields (#317).
import { useEffect, useState } from "react";

function* gen(s: (v: number) => void) {
  yield s;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    Array.from(gen(setN)).forEach((f) => f(n + 1));
  });
  return <div>{n}</div>;
}
