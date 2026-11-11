// oracle: react=loop
// #252: the guard `!s.ok` is dead under the write `{ ok: true }`, but the
// first effect writes `false` back into the object in place on every render,
// so the guard holds again on every new object the second effect stores.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ ok: false });
  useEffect(() => {
    s.ok = false;
  });
  useEffect(() => {
    if (!s.ok) setS({ ok: true });
  }, [s]);
  return <div>{String(s.ok)}</div>;
}
