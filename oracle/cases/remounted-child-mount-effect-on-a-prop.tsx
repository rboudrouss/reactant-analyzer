// oracle: react=loop
// #162 (2), conditional: the remounting child also sits behind a prop, so the
// loop depends on the parent and the claim is a Warning, never an Error.
import { useEffect, useState } from "react";

function Child({ onReady }: { onReady: (v: boolean) => void }) {
  useEffect(() => {
    onReady(false);
  }, []);
  return null;
}

export function App({ show = true }: { show?: boolean }) {
  const [s, setS] = useState(true);
  useEffect(() => {
    if (!s) setS(true);
  }, [s]);
  return <div>{show && s && <Child onReady={setS} />}</div>;
}
