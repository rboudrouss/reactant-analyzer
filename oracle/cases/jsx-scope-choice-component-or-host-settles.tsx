// oracle: react=converges
// #392. `Comp` is `Quiet` or a host button: the element is either. `Quiet`
// calls the prop only on a click, and a host button holds `onClick` as a
// listener: nothing writes the state while rendering.
import { useState } from "react";

function Quiet(p: { onClick: () => void }) {
  return <button onClick={p.onClick}>q</button>;
}

export function App() {
  const [s, setS] = useState(0);
  const Comp = s > 5 ? Quiet : "button";
  return (
    <div>
      {s}
      <Comp onClick={() => setS(s + 1)} />
    </div>
  );
}
