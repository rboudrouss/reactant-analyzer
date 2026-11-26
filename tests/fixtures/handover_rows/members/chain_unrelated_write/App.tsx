import { useState } from 'react';
import ext from 'ext';

// `chain_intact` plus an unrelated write of a function into an object that is
// no package import (the global object): it replaces no package's export, so
// the member chain is still the package's. An Info.
(window as any).onfoo = () => 0;

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
