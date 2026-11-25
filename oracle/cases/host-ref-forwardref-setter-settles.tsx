// oracle: react=converges
// #412. A parent's setter handed as the `ref` of a `forwardRef` child that
// puts it on its host element: one function for the parent's life, so React
// attaches it once, at mount. The analysis does not prove the child's `ref`
// stable across the parent's renders and keeps a Warning (an `fp`).
import { forwardRef, useState } from "react";

const Panel = forwardRef<HTMLDivElement, { label: string }>(function Panel(p, ref) {
  return <div ref={ref}>{p.label}</div>;
});
Panel.displayName = "Panel";

export function App() {
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  return <Panel label={String(!!el)} ref={setEl} />;
}
