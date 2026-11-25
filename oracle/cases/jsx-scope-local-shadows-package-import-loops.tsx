// oracle: react=loop
// #392. A local of the component shadows a package import of the same name:
// `<D/>` renders the program's `Loud`, which calls the prop in its render.
// The package's `ClickCalls` is not the component that runs here, so the
// hand-over is no "package code" Info.
import { useState } from "react";
import { ClickCalls as D } from "ext";

function Loud(p: { fn: (n: number) => void }) {
  p.fn(Math.random());
  return null;
}

export function App() {
  const [s, setS] = useState(0);
  const D = Loud;
  return (
    <div>
      {s}
      <D fn={(x: number) => setS(x)} />
    </div>
  );
}
