// oracle: react=loop
// #335 (the program half of #311): a constructor that returns an object
// makes it what `new` evaluates to; its `run` calls the setter.
import { useMemo, useState } from "react";

class K {
  constructor(set) {
    return {
      run: () => {
        set((c) => c + 1);
        return 1;
      },
    };
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = useMemo(() => new K(setN), []);
  const v = k.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
