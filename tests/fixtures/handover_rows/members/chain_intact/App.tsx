import { useState } from 'react';
import ext from 'ext';
import * as ns from 'ext';

// Members of a default and of a namespace import, in a program that writes no
// function into an object it does not allocate: Infos. A primitive written on
// a foreign object installs no callee.
(globalThis as any).flag = 1;

export function App() {
  const [s, setS] = useState(0);
  const [t, setT] = useState(0);
  const box = { f: () => 0 };
  box.f = () => 1;
  ext.runNow(() => setS(s + 1));
  ns.runNow(() => setT(t + 1));
  return <div>{s + t + box.f()}</div>;
}
