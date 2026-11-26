// oracle: react=converges
// The written `ext` is a parameter that shadows the package import: the write
// installs a function into the object the caller passes, not into the
// package's (#403). The package's component calls the setter from a button's
// `onClick` only: React settles, and the hand-over row is an Info (`tn`).
import { useState } from "react";
import ext from "ext";

function install(ext: any) {
  ext.hook = (f: () => void) => f();
}
install({});

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ext.ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
