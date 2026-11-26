import { useState } from 'react';
import { runNow } from 'ext';

// Plain package callee, no write of the program on a foreign object: an Info.
export function App() {
  const [s, setS] = useState(0);
  runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
