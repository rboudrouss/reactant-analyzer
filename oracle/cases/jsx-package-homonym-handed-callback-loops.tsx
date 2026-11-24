// @file App.tsx
// oracle: react=loop
// oracle: known=fn #325
// #389. `Trans` is imported from a package (React's `Profiler` under another
// name): code the analysis does not see, which calls `onRender` after every
// commit. The program defines a `Trans` of its own, which never calls it;
// running that body in the package's place dropped the loop silently. The
// element is now an unresolved child, disclosed as an `analysis-limit`; the
// closure it is handed writes the state with no row yet (#325).
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
