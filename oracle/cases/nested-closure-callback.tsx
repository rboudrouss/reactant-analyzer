// oracle: react=loop
// #246: a call-returned setter called in an arrow nested in a `useCallback`
// body, the callback called by a no-deps effect.
import { useCallback, useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  const bump = useCallback(() => {
    [1].forEach(() => get()(v + 1));
  }, [v]);
  useEffect(() => {
    bump();
  });
  return <div>{v}</div>;
}
