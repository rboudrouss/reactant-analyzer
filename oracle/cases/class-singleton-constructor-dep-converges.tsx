// oracle: react=converges
// #311, the program half (#335): the constructor returns the instance a
// module binding keeps from the second `new` on, so `o` holds still after
// the first render and the effect settles. Not a new reference at every
// render.
import { useEffect, useState } from "react";

let instance = null;

class Service {
  constructor() {
    if (instance) return instance;
    instance = this;
  }
}

export function App() {
  const [s, setS] = useState(0);
  const o = new Service();
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
