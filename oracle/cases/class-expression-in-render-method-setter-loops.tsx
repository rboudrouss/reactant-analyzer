// oracle: react=loop
// A class expression bound in the render body: a method of its instance calls
// the setter in render.
import { useState } from "react";
export function App() {
  const [n, setN] = useState(0);
  const X = class {
    run() {
      setN(n + 1);
    }
  };
  const x = new X();
  x.run();
  return <div>{n}</div>;
}
