// oracle: react=loop
// #258: the first effect assigns the render's `ready`, the second reads it in
// the same commit. Each effect body runs from the render's exit env, where
// `ready` is `false`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  let ready = false;
  useEffect(() => {
    ready = true;
  });
  useEffect(() => {
    if (ready) setN(n + 1);
  });
  return <div>{n}</div>;
}
