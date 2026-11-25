// oracle: react=converges
// #392. A local `const` of the component aliases a package's member: the
// element is the package's `ClickCalls`, as `<Ext.ClickCalls/>` written in
// place is (#404's module alias, in a body), which calls `fn` only on a click.
import { useState } from "react";
import * as Ext from "ext";

export function App() {
  const [s, setS] = useState(0);
  const D = Ext.ClickCalls;
  return (
    <div>
      {s}
      <D fn={() => setS(s + 1)} />
    </div>
  );
}
