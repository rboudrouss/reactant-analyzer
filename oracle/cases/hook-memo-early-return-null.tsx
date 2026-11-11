// oracle: react=loop
// #240, through a custom hook: the memoized tuple the hook returns is `null`
// on an early exit. That return dropped the tuple's sites for the whole memo,
// so the caller's `t[1]` was a call of ⊤ and the effect's write was missed.
import { useEffect, useMemo, useState } from "react";

function useSlot(c: boolean | undefined) {
  const [v, setV] = useState(0);
  return useMemo(() => {
    if (c) return null;
    return [v, setV] as const;
  }, [c, v]);
}

export function App(props: { c?: boolean }) {
  const t = useSlot(props.c);
  useEffect(() => {
    if (t) t[1](t[0] + 1);
  });
  return <div>{t ? t[0] : 0}</div>;
}
