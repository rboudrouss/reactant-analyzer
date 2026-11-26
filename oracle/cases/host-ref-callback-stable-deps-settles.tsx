// oracle: react=converges
// #412. A `useCallback` ref whose deps never move is one function for the
// component's life: React calls it once, at mount, although it stores a new
// object.
import { useCallback, useState } from "react";

export function App() {
  const [w, setW] = useState<unknown>(0);
  const measure = useCallback((el: HTMLDivElement | null) => {
    setW({ el });
  }, []);
  return <div ref={measure}>{String(!!w)}</div>;
}
