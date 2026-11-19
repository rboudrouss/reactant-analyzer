// oracle: react=converges
// The callback writes a constant: the second write stores what the slot
// holds, and React bails out. A possible loop for the analysis (#320).
import { useState, useCallback, useEffect } from "react";

function C0({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

export function App() {
  const [s0, setS0] = useState(1);
  const cb3 = useCallback((_x: number) => {
    setS0(0);
  }, []);
  return (
    <div>
      {s0}
      <C0 on0={cb3} p1={s0 + 2} />
    </div>
  );
}
