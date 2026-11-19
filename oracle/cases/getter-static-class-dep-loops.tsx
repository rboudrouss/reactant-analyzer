// oracle: react=loop
// #334. A static getter: `K.p` runs it at every render, a new object each
// time.
import { useEffect, useState } from "react";

class K {
  static get p() {
    return {};
  }
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [K.p]);
  return <div>{n}</div>;
}
