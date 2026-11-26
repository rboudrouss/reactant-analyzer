// oracle: react=loop
// #412. A fresh ref callback returning a cleanup (React 19): at each commit
// React runs the old callback's cleanup instead of calling it with `null`
// (`safelyDetachRef`), then attaches the new one, which stores a new object.
import { useState } from "react";

export function App() {
  const [w, setW] = useState<unknown>(0);
  return (
    <div
      ref={(el) => {
        setW({ el });
        return () => {};
      }}
    >
      {String(!!w)}
    </div>
  );
}
