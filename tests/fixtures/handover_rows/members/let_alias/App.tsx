import { useState } from 'react';
import ext from 'ext';

// A `let` bound to a package's component may be reassigned to program code
// (here it is): only a `const` aliases a package's member (#404). A Warning.
function Local({ fn }: { fn: () => void }) {
  fn();
  return null;
}
let Comp: any = ext.Comp;
Comp = Local;

export function App() {
  const [n, setN] = useState(0);
  return <Comp fn={() => setN(n + 1)} />;
}
