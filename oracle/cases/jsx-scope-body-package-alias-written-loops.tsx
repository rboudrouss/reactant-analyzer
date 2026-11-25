// oracle: react=loop
// #392. A local `const` aliases a package's member that calls `fn` after every
// commit, and the program writes a function into it through the alias: as for
// a module alias (#404), that write may install program code in the package's
// export, so the hand-over is no "package code" Info but a Warning.
import { useState } from "react";
import * as Ext from "ext";

export function App() {
  const [s, setS] = useState(0);
  const D = Ext.EffectCalls;
  (D as any).describe = () => "effect calls";
  return (
    <div>
      {s}
      <D fn={() => setS(s + 1)} />
    </div>
  );
}
