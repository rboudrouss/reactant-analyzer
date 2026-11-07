// oracle: react=loop
// #268: the closure is only `useRef`'s argument, never reassigned; the
// effect calls it after every commit, and its functional update moves the
// slot each time.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef(() => setN((c) => c + 1));
  useEffect(() => {
    ref.current();
  });
  return <div>{n}</div>;
}
