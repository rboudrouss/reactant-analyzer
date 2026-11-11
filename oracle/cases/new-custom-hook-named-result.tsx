// oracle: react=loop
// #233: a hook's result kept whole, not destructured. Its tuple got no heap
// object, and a call through `t[1]` named no setter the writer relation
// knew, so the loop read as `verified`.
import { useEffect, useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return [v, setV] as const;
}

export function App() {
  const t = useSlot();
  useEffect(() => {
    t[1](t[0] + 1);
  });
  return <div>{t[0]}</div>;
}
