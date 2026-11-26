import { useState } from 'react';
import ext from 'ext';

// Primitive literals written into a package import hold no function, so they
// install no program code there (#403): the member chain is still the
// package's. An Info.
(ext as any).flag = true;
(ext as any).inner.depth = 10;
(ext as any).mode = 'sync';

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
