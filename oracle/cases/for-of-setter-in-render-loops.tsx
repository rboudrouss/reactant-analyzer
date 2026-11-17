// oracle: react=loop
// The render calls the setter a `for…of` variable holds.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  for (const set of [setN]) set(n + 1);
  return <div>{n}</div>;
}
