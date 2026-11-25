// @file App.tsx
// oracle: react=loop
// #392. A polymorphic `as` prop: `Box` renders whatever its parent passes,
// here `Loud`, which calls the prop in its render with a new value each time.
// `Box`'s `<Comp/>` is that value, not a component named `Comp`.
import { useState } from "react";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return <i>l</i>;
}

function Box({ as: Comp = "div", ...rest }: { as?: any; onPick: (n: number) => void }) {
  return <Comp {...rest} />;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <Box as={Loud} onPick={(x: number) => setS(x)} />
    </div>
  );
}

// @file Comp.tsx
export function Comp() {
  return <i>a component named like the binding, which never runs</i>;
}
