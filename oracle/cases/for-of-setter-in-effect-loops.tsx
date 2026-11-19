// oracle: react=loop
// A `for…of` variable is an element of the iterated value, as an array
// method's callback parameter is: the setter it holds writes the slot.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const set of [setN]) set(n + 1);
  });
  return <div>{n}</div>;
}
