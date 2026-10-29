// oracle: react=converges
// #248: `sort` on a one-entry array calls no comparator, and `reduce` with
// no initial value calls no reducer: neither write runs, so neither is a
// certain render write.
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  [1].sort(() => {
    setV(v + 1);
    return 0;
  });
  [1].reduce((acc) => {
    setV(v + 1);
    return acc;
  });
  return <div>{v}</div>;
}
