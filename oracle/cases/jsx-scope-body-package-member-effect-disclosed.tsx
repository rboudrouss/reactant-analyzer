// @file App.tsx
// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents fn
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers fn
// (Under `packagePropsRunOnEvents` the package's member calls `fn` from an
// effect, against the premise: the loop is the assumption's price, ADR-048.)
// #392. A local `const` of the component aliases a package's member that
// calls `fn` after every commit: the element is that package's code, as
// `<Ext.EffectCalls/>` written in place is, so the setter handed to it is an
// Info hand-over to package code (`disclosed`), not a same-named program
// component's.
import { useState } from "react";
import * as Ext from "ext";

export function App() {
  const [s, setS] = useState(0);
  const D = Ext.EffectCalls;
  return (
    <div>
      {s}
      <D fn={() => setS(s + 1)} />
    </div>
  );
}

// @file EffectCalls.tsx
export function EffectCalls() {
  return <i>a program component named like the package's, which never runs</i>;
}
