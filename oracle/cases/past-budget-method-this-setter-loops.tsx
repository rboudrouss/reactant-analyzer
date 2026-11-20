// oracle: react=loop
// A method call past the inlining budget (#340): `o.run` is not run there but
// escapes, and runs later with `this` unknown. Its body reads its receiver,
// so the receiver is handed over at the call, with the setter it holds.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    run() {
      this.set((c: number) => c + 1);
      return 1;
    },
  };
  const a = () => o.run();
  const b = () => a();
  const c = () => b();
  const v = c();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
