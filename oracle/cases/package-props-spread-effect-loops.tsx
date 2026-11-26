// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents disclosed
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers disclosed
// The function reaches the package through a spread, which names no prop at
// the element (and forwards `ref` and `children` in React 19): the package
// calls it from an effect after every commit. `packagePropsRunOnEvents` covers
// named props only: the Info hand-over stays (`disclosed` both ways).
import { useState } from "react";
import { EffectCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const props = { fn: () => setN(n + 1) };
  return (
    <div>
      {n}
      <EffectCalls {...props} />
    </div>
  );
}
