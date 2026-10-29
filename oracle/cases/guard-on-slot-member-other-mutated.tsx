// oracle: react=converges
// #252, the precision side: the in-place write moves another member than the
// one the guard tests, so `!s.ok` stays dead under the write `{ ok: true }`.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ ok: false, n: 0 });
  useEffect(() => {
    s.n = 1;
  });
  useEffect(() => {
    if (!s.ok) setS({ ok: true, n: 0 });
  }, [s]);
  return <div>{String(s.ok)}</div>;
}
