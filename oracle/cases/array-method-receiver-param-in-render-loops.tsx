// oracle: react=loop
// An array method passes its callback the receiver itself, third.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [setN].forEach((set, i, arr) => arr[i](n + 1));
  return <div>{n}</div>;
}
