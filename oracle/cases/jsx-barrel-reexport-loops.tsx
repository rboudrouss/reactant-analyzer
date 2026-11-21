// @file App.tsx
// oracle: react=loop
// #389. `Trans` is imported through a barrel: the import settles it, through
// the barrel's re-export, to the component that loops — the child the parent
// renders and hands its setter, not a root of its own.
import { useState } from "react";
import { Trans } from "./ui";

export function App() {
  const [n, setN] = useState(0);
  return <Trans n={n} onTick={setN} />;
}

// @file ui.tsx
export * from "./Trans";

// @file Trans.tsx
import { useEffect } from "react";

export function Trans({ n, onTick }: { n: number; onTick: (n: number) => void }) {
  useEffect(() => {
    onTick(n + 1);
  });
  return <div>{n}</div>;
}
