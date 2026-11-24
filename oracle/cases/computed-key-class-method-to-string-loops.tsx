// oracle: react=loop
// #370: a computed method name of a class converts its expression to a
// property key where the class is defined — here in the render — which calls
// the `toString` the key object holds, with the setter.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const k = {
    toString: () => {
      setN(n + 1);
      return "m";
    },
  };
  class K {
    [k as unknown as string]() {
      return 1;
    }
  }
  return (
    <div>
      {n}
      {typeof K}
    </div>
  );
}
