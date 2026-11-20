// oracle: react=loop
// #373: a write through a setter past the inlining budget does not run the
// setter there; it runs later, handed the value written, as a method past the
// budget is handed its arguments. Here it calls the closure written, with the
// setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set run(f: () => void) {
      f();
    },
  };
  const a = () => {
    o.run = () => setN(n + 1);
  };
  const b = () => a();
  const c = () => b();
  c();
  return <div>{n}</div>;
}
