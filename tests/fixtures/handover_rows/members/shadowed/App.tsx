import { useState } from 'react';
import { runNow as ext } from 'ext';

// A parameter shadows the package import: the callee is the parent's closure.
// A Warning.
function Child({ ext, setS, s }: { ext: (f: () => void) => void; setS: (n: number) => void; s: number }) {
  ext(() => setS(s + 1));
  return null;
}
export function App() {
  const [s, setS] = useState(0);
  return <div>{s}<Child ext={(f) => f()} setS={setS} s={s} /></div>;
}
