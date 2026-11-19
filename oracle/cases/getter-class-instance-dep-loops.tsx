// oracle: react=loop
// #321. A class getter that allocates, read off an instance the render
// builds: the analysis does not place the instance, so the read is unknown
// and the effect may re-run after every render.
import { useEffect, useState } from "react";

class K {
  get p() {
    return {};
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = new K();
  useEffect(() => {
    setN((c) => c + 1);
  }, [k.p]);
  return <div>{n}</div>;
}
