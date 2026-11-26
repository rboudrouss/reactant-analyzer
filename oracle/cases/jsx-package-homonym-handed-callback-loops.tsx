// @file App.tsx
// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents fn
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers fn
// (Under `packagePropsRunOnEvents` the premise is false for React's own
// `Profiler`, which calls `onRender` at every commit: the loop is the
// assumption's price, ADR-048.)
// #389. `Trans` is imported from a package (React's `Profiler` under another
// name): code the analysis does not see, which calls `onRender` after every
// commit. The program defines a `Trans` of its own, which never calls it;
// running that body in the package's place dropped the loop silently. The
// element is now an unresolved child, disclosed as an `analysis-limit`, and
// the closure it is handed runs there at an unknown time (#325).
import { useState, Profiler as Trans } from "react";

export function App() {
  const [n, setN] = useState(0);
  return (
    <Trans id="t" onRender={() => setN(n + 1)}>
      <div>{n}</div>
    </Trans>
  );
}

// @file sdk.tsx
export function Trans({ children }: { id: string; onRender: () => void; children?: any }) {
  return <span>{children}</span>;
}
