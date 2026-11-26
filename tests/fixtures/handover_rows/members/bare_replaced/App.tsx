import { useState } from 'react';
import ext, { format } from 'ext';

// A bare imported name whose module member the program replaced through the
// default import: module interop reads `format` off that object. A Warning.
(ext as any).format = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  format(() => setS(s + 1));
  return <div>{s}</div>;
}
