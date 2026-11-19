// oracle: react=loop
// #321. Destructuring reads the member through the getter too.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = { get p() { return {}; } };
  const { p } = o;
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}
