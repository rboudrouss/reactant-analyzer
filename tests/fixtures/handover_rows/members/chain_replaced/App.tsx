import { useState } from 'react';
import ext from 'ext';

// The program writes its own function on a package object, then calls it:
// the callee is program code, React loops. A Warning.
ext.hook = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  ext.hook(() => setS(s + 1));
  return <div>{s}</div>;
}
