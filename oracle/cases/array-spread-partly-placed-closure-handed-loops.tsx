// oracle: react=loop
// #363, the array neighbour: a spread of an array literal or of a value the
// analysis does not place (a prop) may hold the literal's closure at any
// position; handed to a hook of a package that calls each function element
// after every commit, it hands the closure over.
import { useState } from "react";
import { useRun } from "ext";

export function App({ c = true, xs }: { c?: boolean; xs?: any }) {
  const [n, setN] = useState(0);
  const fs = [...(c ? [() => setN(n + 1)] : xs)];
  useRun(fs);
  return <div>{n}</div>;
}
