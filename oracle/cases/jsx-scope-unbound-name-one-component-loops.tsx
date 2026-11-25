// @file App.tsx
// oracle: react=loop
// #392. A name bound nowhere in the file is a global: here the one program
// component of that name, which its file installs on `globalThis`. The
// by-name reading serves only this, and still resolves it: its effect bumps
// the parent's state after every render.
import { useState } from "react";
import "./Bump";

export function App() {
  const [n, setN] = useState(0);
  // @ts-ignore a global
  return <Bump n={n} onTick={setN} />;
}

// @file Bump.tsx
import { useEffect } from "react";

export function Bump({ n, onTick }: { n: number; onTick: (n: number) => void }) {
  useEffect(() => {
    onTick(n + 1);
  });
  return <div>{n}</div>;
}
(globalThis as any).Bump = Bump;
