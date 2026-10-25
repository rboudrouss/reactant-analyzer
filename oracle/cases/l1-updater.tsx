// oracle: react=loop
// oracle: known=fn #168
// L1: a functional updater in an effect keyed on the slot it writes.
import { useEffect, useState } from "react";

export function App() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount((c) => c + 1);
  }, [count]);
  return <p>{count}</p>;
}
