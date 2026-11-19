// oracle: react=loop
// A getter of a literal a mount-only memo builds runs at every read of its
// member: the render's read is a call of it, in the render (#337).
import { useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = useMemo(
    () => ({
      get p() {
        setN((c) => c + 1);
        return 1;
      },
    }),
    [],
  );
  const v = o.p;
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
