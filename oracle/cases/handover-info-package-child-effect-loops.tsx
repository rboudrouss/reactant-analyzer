// oracle: react=loop
// A setter handed to a package's component the analysis does not see, which
// calls it from an effect after every commit: the write re-renders `App`, which
// re-renders the child, whose effect runs again. The analyzer cannot place the
// call, so its row is the Info hand-over of `setter-in-render` (a limit of the
// analysis, behind `--info`), not a loop claim: the verdict is `disclosed`.
import { useState } from "react";
import { EffectCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <EffectCalls fn={() => setN(n + 1)} />
    </div>
  );
}
