// @file App.tsx
// oracle: react=loop
// The neighbour of `handover-package-alias-element-handler-settles`: the
// `const` aliases a member of a PROGRAM object, whose component calls the
// prop during its render. React loops; the alias is no package's (#404), so
// the element stays a program child the analysis does not resolve and the
// hand-over is a Warning (`tp`).
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
const lib = {
  Comp: ({ fn }: { fn: () => void }) => {
    fn();
    return null;
  },
};

export const Comp = lib.Comp;
