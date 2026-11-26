// oracle: react=converges
// A call named like a reflective writer whose first argument is the package
// object, but without the argument that would hold what it writes: read as
// `Reflect.set(ext, k)` it writes `undefined`, so it installs no code and does
// not count (#403). Here it is a `Map`'s `set`. The package's component calls
// the setter on a click only: React settles, the row is an Info (`tn`).
import { useState } from "react";
import ext from "ext";

const seen = new Map();
seen.set(ext, true);

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ext.ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
