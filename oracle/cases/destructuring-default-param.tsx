// oracle: react=loop
// oracle: known=fn #256
// A destructured parameter's default is the member when the argument does
// not hold it; its value is not stored yet.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = ({ f = () => setN(n + 1) }: { f?: () => void }) => f();
  useEffect(() => {
    run({});
  });
  return <div>{n}</div>;
}
