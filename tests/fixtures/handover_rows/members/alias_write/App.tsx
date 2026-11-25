import { useState } from 'react';
import ext from 'ext';

// A module `const` aliasing the import is the import (#404): a write through
// it is a write into the package's object, which may install program code
// there. React loops; the row is a Warning.
const e = ext as any;
e.hook = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
