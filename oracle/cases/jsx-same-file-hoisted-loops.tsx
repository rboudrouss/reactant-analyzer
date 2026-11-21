// oracle: react=loop
// #389. `Trans` is this file's own, defined after its use and imported from
// nowhere: the element still names it. Its effect bumps the parent's state
// after every render. (A homonym in another file would make the claim's
// component `Trans@file`, which the oracle's render count, keyed by function
// name, does not match; the registry's tests pin the homonym.)
import { useState, useEffect } from "react";

export function App() {
  const [n, setN] = useState(0);
  return <Trans n={n} onTick={setN} />;
}

function Trans({ n, onTick }: { n: number; onTick: (n: number) => void }) {
  useEffect(() => {
    onTick(n + 1);
  });
  return <div>{n}</div>;
}
