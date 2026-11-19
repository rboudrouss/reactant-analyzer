// oracle: react=loop
// oracle: known=fn #340
// A static method's `this` is the class it is called off (#338). The static
// member is kept under a synthetic key, so `K.run` resolves to no body, and
// the receiver is not handed to it.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  class K {
    static set = setN;
    static run() {
      this.set((c: number) => c + 1);
      return 1;
    }
  }
  const v = K.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
