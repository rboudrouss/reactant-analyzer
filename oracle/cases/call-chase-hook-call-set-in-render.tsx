// oracle: react=loop
// #46: a hook call's setter member called directly, `useThing().set(x)`, in render.
import { useState } from "react";

function useThing() {
  const [v, setV] = useState(0);
  return { v, set: setV };
}

export function App() {
  useThing().set((x) => x + 1);
  return <div />;
}
