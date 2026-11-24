// oracle: react=loop
// The standing premise, stated in docs/limitations.md ("A name imported from a
// package denotes that package's export"): the program replaces a package
// export through an ALIAS of the import, which the fact does not see, as the
// registry's own summaries do not see a replaced `React.useCallback`. React
// loops through program code, and the analyzer reports the hand-over as an
// Info: `disclosed`. This is the one known shape where an Info row covers a
// loop whose callee is the program's.
import { useState } from "react";
import ext from "ext";

const e = ext as any;
e.hook = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
