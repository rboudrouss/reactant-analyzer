// oracle: react=loop
// #268: the "latest callback" hook (`useLatest`, a `useEvent` polyfill) keeps
// its argument in a ref and hands the ref back; the caller's effect calls
// through it after every commit.
import { useEffect, useRef, useState } from "react";

function useLatest<T>(value: T) {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

export function App() {
  const [n, setN] = useState(0);
  const latest = useLatest(() => setN(n + 1));
  useEffect(() => {
    latest.current();
  });
  return <div>{n}</div>;
}
