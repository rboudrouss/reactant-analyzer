import { useState } from 'react';

// `Array.isArray` reads nothing of its argument and calls no code
// (ECMA-262 §7.2.2): the closure is handed to no one (#391). No row.
export function App() {
  const [s, setS] = useState(0);
  const cb = () => setS(s + 1);
  return <div>{String(Array.isArray(cb))}</div>;
}
