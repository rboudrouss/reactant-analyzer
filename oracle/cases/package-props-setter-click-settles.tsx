// oracle: react=converges
// oracle: assume=packagePropsRunOnEvents tn
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers tn
// A bare setter and an updater closure handed to a package component that
// calls them from a click only: no event fires, React settles. Info rows by
// default (`tn`); none under `packagePropsRunOnEvents`.
import { useState } from "react";
import { ClickCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ClickCalls fn={() => setN((c) => c + 1)} />
      <ClickCalls fn={setN} />
    </div>
  );
}
