// oracle: react=converges
// #252: the in-place write sits in a handler only. No event runs, so React
// settles; the click would revive the guard on the next run of the effect,
// so the analysis may warn, and never certifies.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ ok: false });
  useEffect(() => {
    if (!s.ok) setS({ ok: true });
  }, [s]);
  return (
    <button
      onClick={() => {
        s.ok = false;
      }}
    >
      {String(s.ok)}
    </button>
  );
}
