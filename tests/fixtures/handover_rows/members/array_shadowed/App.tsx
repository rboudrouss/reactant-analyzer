import { useState } from 'react';

// A parameter named `Array` is no builtin (#391): the closure is handed to
// whatever the caller passes, a Warning.
export function check(Array: { isArray: (f: () => void) => boolean }, f: () => void) {
  return Array.isArray(f);
}

export function App({ lib }: { lib: { isArray: (f: () => void) => boolean } }) {
  const [s, setS] = useState(0);
  return <div>{String(check(lib, () => setS(s + 1)))}</div>;
}
