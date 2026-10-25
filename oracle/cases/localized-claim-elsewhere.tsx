// oracle: react=loop
// oracle: known=fn #168
// Found by `oracle fuzz --seed 1` (program 179, shrunk while keeping "a claim,
// but not on the loop"). `C0` loops through a functional updater (#168, the
// shape of l1-updater.tsx); the only claim is a Warning on `App`'s dead
// render-phase write. P1 asks for a claim on a component React re-renders in
// the loop, so this is an fn, not the tp a whole-program check would count.
import { useEffect, useState } from "react";

function C0() {
  const [s, setS] = useState(2);
  useEffect(() => {
    setS((prev) => prev - 3);
  });
  return <div />;
}

export function App() {
  const [s, setS] = useState(0);
  if (0) {
    setS(0);
  }
  return (
    <div>
      <C0 />
    </div>
  );
}
