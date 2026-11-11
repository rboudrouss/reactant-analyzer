// oracle: react=loop
// #284, the `useCallback` variant of closure-setter-alias-in-render.tsx: the
// render calls the memoized callback, whose own `s` is the setter.
import { useCallback, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const cb = useCallback(() => {
    const s = setN;
    s(1);
  }, []);
  cb();
  return <div>{n}</div>;
}
