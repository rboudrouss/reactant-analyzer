// oracle: react=loop
// #207. `Mid` is re-keyed by `App`'s slot at every render, and `Child` is
// remounted with it: its `[]` effect fires at every mount and writes the slot
// again. A component that does not stay mounted runs its mount effects once
// per remount (mount edges).
import { useEffect, useState } from "react";

function Child({ onMount }: { onMount: () => void }) {
  useEffect(() => {
    onMount();
  }, []);
  return null;
}

function Mid({ onMount }: { onMount: () => void }) {
  return <Child onMount={onMount} />;
}

export function App() {
  const [k, setK] = useState(false);
  return <Mid key={String(k)} onMount={() => setK(!k)} />;
}
