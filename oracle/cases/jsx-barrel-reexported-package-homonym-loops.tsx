// @file App.tsx
// oracle: react=loop
// oracle: known=fn #325
// #389. The barrel re-exports a package's component (React's `Profiler`, as
// `Trans`), which calls `onRender` after every commit. The import leads to
// code the analysis does not see, never to the program's own `Trans`, which
// does not call it: an unresolved child, disclosed as an `analysis-limit`,
// whose handed closure writes the state with no row yet (#325).
import { useState } from "react";
import { Trans } from "./ui";

export function App() {
  const [n, setN] = useState(0);
  return (
    <Trans id="t" onRender={() => setN(n + 1)}>
      <div>{n}</div>
    </Trans>
  );
}

// @file ui.tsx
export { Profiler as Trans } from "react";

// @file sdk.tsx
export function Trans({ children }: { id: string; onRender: () => void; children?: any }) {
  return <span>{children}</span>;
}
