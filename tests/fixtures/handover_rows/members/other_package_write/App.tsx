import { useState } from 'react';
import ext from 'ext';
import other from 'other-pkg';

// The mirror of `chain_unrelated_write`: the unrelated write is into another
// package's import. The fact is program-wide, so no import reads as a
// package's any more. A Warning.
(other as any).flag = () => 0;

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
