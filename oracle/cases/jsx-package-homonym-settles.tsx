// @file App.tsx
// oracle: react=converges
// #389. `Trans` is imported from a package (React's `Profiler` under another
// name), which calls `onRender` with its `id` after each commit: the state
// takes `"t"` once and React bails out. The program's own `Trans` would call
// the setter during render with a new value at every render; it is not the
// component that runs here, so no Error may rest on it.
import { useState, Profiler as Trans } from "react";

export function App() {
  const [v, setV] = useState<unknown>("");
  return (
    <Trans id="t" onRender={setV}>
      <div>{String(v)}</div>
    </Trans>
  );
}

// @file sdk.tsx
export function Trans({ onRender, children }: { id: string; onRender: (v: unknown) => void; children?: any }) {
  onRender({});
  return <span>{children}</span>;
}
