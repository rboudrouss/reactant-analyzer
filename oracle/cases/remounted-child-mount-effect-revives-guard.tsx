// oracle: react=loop
// #162 (2). `Child` is mounted under the slot `App`'s effect restores: its `[]`
// effect clears the slot, which unmounts it, the effect sets the slot back,
// which mounts it again. A mount-only effect of a child that does not stay
// mounted is a site of the slot it writes.
import { useEffect, useState } from "react";

function Child({ onReady }: { onReady: (v: boolean) => void }) {
  useEffect(() => {
    onReady(false);
  }, []);
  return null;
}

export function App() {
  const [s, setS] = useState(true);
  useEffect(() => {
    if (!s) setS(true);
  }, [s]);
  return <div>{s && <Child onReady={setS} />}</div>;
}
