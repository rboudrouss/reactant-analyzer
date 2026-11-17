import { useCallback, useState } from "react";
import { C0 } from "./child";

export function App() {
  const [s0, setS0] = useState(1);
  const cb3 = useCallback((x: number) => {
    setS0(x);
  }, []);
  return (
    <div>
      {s0}
      <C0 on0={cb3} p1={s0 + 2} />
    </div>
  );
}
