// oracle: react=loop
// #360, #337: a mount-only memo builds a literal whose `toString` is a
// getter: every conversion of it reads the member, which runs the getter,
// with the setter, in the render.
import { useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = useMemo(
    () => ({
      get toString() {
        setN((c) => c + 1);
        return () => "";
      },
    }),
    [],
  );
  const s = `${o}`;
  return <div>{n}{s}</div>;
}
