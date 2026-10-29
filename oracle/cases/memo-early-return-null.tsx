// oracle: react=loop
// #240: a memo that returns `null` on an early exit kept no site for its
// object, so `api.setV` was a call of ⊤ and the effect's write was missed.
import { useEffect, useMemo, useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  const api = useMemo(() => {
    if (props.c) return null;
    return { v, setV };
  }, [props.c, v]);
  useEffect(() => {
    if (api) api.setV(api.v + 1);
  });
  return <div>{v}</div>;
}
