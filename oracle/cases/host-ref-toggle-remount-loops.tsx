// oracle: react=loop
// #412. A stable ref callback (the setter) on an element whose presence the
// written state decides: the element unmounts (`null`) and mounts again (a
// new node) at every other commit. Its mount condition is part of the
// commit's deps.
import { useState } from "react";

export function App() {
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  return <section>{el ? null : <div ref={setEl}>x</div>}</section>;
}
