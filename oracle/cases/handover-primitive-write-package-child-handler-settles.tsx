// oracle: react=converges
// The program writes primitive literals into the package's object
// (`MotionGlobalConfig.skipAnimations = true`, `inspect.defaultOptions.depth
// = 10`): a primitive holds no function, so it installs no program code there
// and the package's component is still the package's (#403). It calls the
// setter from a button's `onClick` only: React settles, and the hand-over row
// is an Info (`tn`); before #403 the write voided every package callee and the
// row was a Warning (`fp`).
import { useState } from "react";
import ext from "ext";

(ext as any).flag = true;
(ext as any).depth = 10;

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ext.ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
