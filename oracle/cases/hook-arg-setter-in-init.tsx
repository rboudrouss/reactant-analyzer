// oracle: react=loop
// A hook's argument is evaluated during render at every render (#193). The
// setter call in the `useState` init ran in no render pass of the analyzer,
// which printed `verified setter-in-render`.
import { useState } from "react";

export function App() {
  const [a, setA] = useState(0);
  const [b] = useState(setA(a + 1));
  return <div>{a}{b}</div>;
}
