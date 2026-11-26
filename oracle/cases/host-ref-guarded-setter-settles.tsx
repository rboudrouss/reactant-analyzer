// oracle: react=converges
// #412. A stable ref callback on an element under a guard the callback does
// not move: the element stays mounted, so React attaches it once.
import { useState } from "react";

export function App() {
  const [open] = useState(true);
  const [anchor, setAnchor] = useState<HTMLDivElement | null>(null);
  return (
    <section>
      {open && <div ref={setAnchor}>{String(!!anchor)}</div>}
    </section>
  );
}
