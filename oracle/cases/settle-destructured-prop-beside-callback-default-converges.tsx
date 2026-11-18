// oracle: react=converges
// #331, the precision side: `Other`'s callback stores its default into its
// own parameter; `App`'s destructured `k` still holds across the passes, so
// one write settles the guard.
import { useCallback, useState } from "react";

export function Other() {
  const f = useCallback(({ k }: { k?: number } = {}) => k, []);
  return <button onClick={() => f()}>other</button>;
}

export function App({ k }: { k?: number }) {
  const [m, setM] = useState("");
  const x = String(k);
  if (m !== x) setM(x);
  return (
    <div>
      {m}
      <Other />
    </div>
  );
}
