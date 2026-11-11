// oracle: react=loop
// Found by `oracle fuzz --seed 2` (programs 976, 752, 1044, 1087, shrunk). The
// render-phase form of new-custom-hook-setter.tsx: "Too many re-renders" in
// React, `verified setter-in-render` from the analyzer until #185.
import { useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return [v, setV];
}

export function App() {
  const [v, setV] = useSlot();
  setV(0);
  return <div>{v}</div>;
}
