import { useState } from 'react';
import ext from 'ext';

// A module `const` aliasing a package's function is that package's callee,
// as `ext.runNow(…)` written in place is (#404). An Info.
const run = ext.runNow;

export function App() {
  const [s, setS] = useState(0);
  run(() => setS(s + 1));
  return <div>{s}</div>;
}
