import { useState } from 'react';
import ext from 'ext';

// `Object.assign` destructured off `Object` and called by its own name
// installs a program function on the package object (#403): the writer is
// known by its name, whatever spells its receiver. A Warning.
const { assign } = Object;
assign(ext, { hook: (f: () => void) => f() });

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
