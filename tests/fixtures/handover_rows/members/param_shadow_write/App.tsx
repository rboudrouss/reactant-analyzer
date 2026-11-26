import { useState } from 'react';
import ext from 'ext';

// The written `ext` is a parameter that shadows the import (#403): the write
// is into the caller's object, not the package's. An Info.
function install(ext: any) {
  ext.hook = (f: () => void) => f();
}
install({});

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
