// oracle: react=loop
// `o` is a new object on either path, so the effect runs after every commit,
// and it stores a fresh array, so every run re-renders. A name bound twice is
// not certified fresh at the call: the re-run is possible, never silence.
import { useEffect, useState } from "react";

export function App({ c }: { c?: boolean }) {
  const [s, setS] = useState<unknown>(0);
  let o;
  if (c) {
    o = { a: 1 };
  } else {
    o = { b: 1 };
  }
  useEffect(() => {
    setS([]);
  }, [o]);
  return <div>{String(s)}</div>;
}
