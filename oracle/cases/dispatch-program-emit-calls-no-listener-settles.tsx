// oracle: react=converges
// #407, N1: the render calls `bus.emit`, a method of an object the render
// builds, which fires nothing. A call the analysis resolves to program
// bodies alone fires no event but through a dispatch in them: the setter
// handed as `onClick` never runs.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    setM(1);
  }, []);
  const bus = {
    emit(_name: string) {
      return 0;
    },
  };
  bus.emit("ping");
  return (
    <button onClick={setN}>
      {m}
      {String(!!n)}
    </button>
  );
}
