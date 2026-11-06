// oracle: react=loop
// #256: a destructured parameter's member is its default when the argument
// does not hold it.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = ({ f = () => setN(n + 1) }: { f?: () => void }) => f();
  useEffect(() => {
    run({});
  });
  return <div>{n}</div>;
}
