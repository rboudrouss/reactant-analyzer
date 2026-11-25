import { useState } from 'react';

// The `const` aliases a member of a program object: no package's code, so
// the element stays a program child the analysis does not resolve (#404). A
// Warning.
const lib = {
  Comp: ({ fn }: { fn: () => void }) => {
    fn();
    return null;
  },
};
const Comp = lib.Comp;

export function App() {
  const [n, setN] = useState(0);
  return <Comp fn={() => setN(n + 1)} />;
}
