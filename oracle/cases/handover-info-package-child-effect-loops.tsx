// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents fn
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers fn
// A setter handed to a package's component the analysis does not see, which
// calls it from an effect after every commit: the write re-renders `App`, which
// re-renders the child, whose effect runs again. The analyzer cannot place the
// call, so its row is the Info hand-over of `setter-in-render` (a limit of the
// analysis, behind `--info`), not a loop claim: the verdict is `disclosed`.
// Under `packagePropsRunOnEvents` the premise is false for this package (it
// calls the prop from an effect, as vaul's uncontrolled `onOpenChange` does):
// the analysis reads the prop as an event's and says nothing, `fn` by the
// assumption — its price (ADR-048).
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
