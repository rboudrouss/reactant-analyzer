// oracle: react=loop
// #412. A host element's `ref` callback, a new function at every render,
// stores a new object: React calls the new callback at every commit
// (`commitAttachRef`), which schedules a render, which commits a new callback.
import { useState } from "react";

export function App() {
  const [w, setW] = useState<unknown>(0);
  return (
    <div
      ref={(el) => {
        setW({ el });
      }}
    >
      {String(!!w)}
    </div>
  );
}
