// oracle: react=loop
// #22: `items` is an array, so `items.slice()` is a new array at every
// render: the effect re-runs after every commit and flips `t`. A state slot's
// kind is not known to the analysis (#312): a possible loop.
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
