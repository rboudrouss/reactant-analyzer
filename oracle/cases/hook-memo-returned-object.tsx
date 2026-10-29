// oracle: react=loop
// #233, the object shape of hook-memo-returned-tuple.tsx:
// `return useMemo(() => ({ v, setV }), [v])` destructured by the caller.
import { useEffect, useMemo, useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return useMemo(() => ({ v, setV }), [v]);
}

export function App() {
  const { v, setV } = useSlot();
  useEffect(() => {
    setV(v + 1);
  });
  return <div>{v}</div>;
}
