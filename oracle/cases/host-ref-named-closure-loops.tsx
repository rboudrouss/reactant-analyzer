// oracle: react=loop
// #412. The same through a name: the `ref` is a closure the render builds
// anew, so React attaches it at every commit.
import { useState } from "react";

export function App() {
  const [w, setW] = useState<unknown>(0);
  const measure = (el: HTMLDivElement | null) => setW({ el });
  return <div ref={measure}>{String(!!w)}</div>;
}
