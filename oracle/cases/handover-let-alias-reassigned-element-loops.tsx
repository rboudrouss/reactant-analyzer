// @file App.tsx
// oracle: react=loop
// A `let` bound to a package's component and then reassigned to a program
// component that calls the prop during its render: React loops. Only a
// `const` aliases a package's member (#404); the `let` stays a program
// binding, and the hand-over is a Warning (`tp`).
import { useState } from "react";
import { Comp } from "./ui";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Comp fn={() => setN(n + 1)} />
    </div>
  );
}

// @file ui.tsx
import ext from "ext";

function Local({ fn }: { fn: () => void }) {
  fn();
  return null;
}

export let Comp: any = ext.ClickCalls;
Comp = Local;
