// oracle: react=loop
// #310: the effect clicks the button, whose handler stores the event it is
// called with: a new object at every click. The handler's parameter `v` is
// its own, not the render's `v`.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [s, setS] = useState<unknown>(0);
  const v = 0;
  const r = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    r.current?.click();
  }, [s]);
  return <button ref={r} onClick={(v) => setS(v)}>x</button>;
}
