// oracle: react=loop
// #233: a closure inside a memoized object, called through its member. The
// memo carried no allocation site, and no binding named `api.inc`.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const api = useMemo(() => ({ inc: () => setV(v + 1) }), [v]);
  useEffect(() => {
    api.inc();
  });
  return <div>{v}</div>;
}
