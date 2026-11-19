// oracle: react=loop
// #335. An arrow a field initializer writes on each instance reads `this`
// as the instance: the render's call of it calls the setter the
// constructor kept.
import { useMemo, useState } from "react";

class K {
  s = null;
  bump = () => {
    this.s((c) => c + 1);
    return 1;
  };
  constructor(set) {
    this.s = set;
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = useMemo(() => new K(setN), []);
  const v = k.bump();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
