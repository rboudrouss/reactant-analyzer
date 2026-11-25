// oracle: react=converges
// #412's settling neighbour: the setter itself as the `ref` is one function
// for the component's life, so React calls it once, at mount.
import { useState } from "react";

export function App() {
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  return <div ref={setEl}>{String(!!el)}</div>;
}
