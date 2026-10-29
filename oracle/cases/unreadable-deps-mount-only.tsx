// oracle: react=converges
// #212. The deps argument is not a written array, so the engine cannot read
// it, but it is a list: here an empty one, and the effect runs once. An
// unreadable list may re-run the effect after every render, never must: a
// Warning is a tolerated FP, an Error would break P2.
import { useEffect, useState } from "react";

const DEPS: unknown[] = [];

export function App() {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
  }, DEPS);
  return <div />;
}
