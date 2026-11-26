// oracle: react=loop
// #412. A `useCallback` ref whose dep is the state it writes: every write
// moves the dep, so the next render hands React a new callback, which it
// attaches at that commit.
import { useCallback, useState } from "react";

export function App() {
  const [w, setW] = useState<unknown>(0);
  const measure = useCallback(
    (el: HTMLDivElement | null) => {
      setW({ el, w });
    },
    [w],
  );
  return <div ref={measure}>{String(!!w)}</div>;
}
