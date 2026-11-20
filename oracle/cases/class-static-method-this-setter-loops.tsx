// oracle: react=loop
// A static method's `this` is the class it is called off (#338). The static
// member is kept under a synthetic key, so `K.run` resolves to no body. The
// class holds no member named `run`, so the method may be its prototype's, a
// builtin the analysis does not model: `K` is handed to it, and the setter it
// holds with it (#330). A Warning.
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
