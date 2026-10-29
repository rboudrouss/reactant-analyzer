// oracle: react=converges
// #252, the precision side: the in-place write stores `true`, the value the
// setter's write already kills the guard with, so `!s.ok` stays dead.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ ok: false });
  useEffect(() => {
    s.ok = true;
  });
  useEffect(() => {
    if (!s.ok) setS({ ok: true });
  }, [s]);
  return <div>{String(s.ok)}</div>;
}
