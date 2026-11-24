import { useState } from 'react';
import { registry } from 'ext';

// The package object holds a program function the program put there through a
// call the analysis does not see; then the program calls what a call of it
// returns: no chain of static members. A Warning.
const run = (f: () => void) => f();
registry.set('run', run);

export function App() {
  const [s, setS] = useState(0);
  registry.get('run')(() => setS(s + 1));
  return <div>{s}</div>;
}
