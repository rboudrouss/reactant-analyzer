// oracle: react=loop
// #22: `items` can only be an array, so `items.slice()` is a new array at
// every render: the effect re-runs after every commit and flips `t`.
import { useEffect, useState } from "react";

export function App() {
  const [items] = useState([3, 1, 2]);
  const [t, setT] = useState(false);
  const copy = items.slice();
  useEffect(() => {
    setT(!t);
  }, [copy]);
  return <div>{String(t)}</div>;
}
