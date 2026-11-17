// oracle: react=loop
// `useEffectEvent` hands back a function that runs its argument (#320).
import { useState, useEffectEvent, useEffect } from "react";

function C0({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

export function App() {
  const [s0, setS0] = useState(1);
  const cb3 = useEffectEvent((x: number) => {
    setS0(x);
  });
  return (
    <div>
      {s0}
      <C0 on0={cb3} p1={s0 + 2} />
    </div>
  );
}
