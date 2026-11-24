import { useState } from 'react';
import ext from 'ext';

// The standing premise (docs/limitations.md): a package export replaced
// through an alias of the import is not seen, as the registry's summaries do
// not see it. React loops; the row is an Info.
const e = ext as any;
e.hook = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
