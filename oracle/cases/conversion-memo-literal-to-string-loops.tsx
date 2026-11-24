// oracle: react=loop
// #360, #337: the literal is built once, by a mount-only memo; its
// `toString` runs at every conversion of it, in the render, with the setter.
import { useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = useMemo(
    () => ({
      toString: () => {
        setN((c) => c + 1);
        return "";
      },
    }),
    [],
  );
  const s = `${o}`;
  return <div>{n}{s}</div>;
}
