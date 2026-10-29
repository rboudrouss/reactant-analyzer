// oracle: react=loop
// #162 (3). The effect's `flag` is its own binding, not the render's `false`:
// the guard holds on every run and the toggle never settles.
import { useEffect, useState } from "react";

export function App({ enabled = true }: { enabled?: boolean }) {
  const [on, setOn] = useState(false);
  const flag = false;
  useEffect(() => {
    const flag = enabled;
    if (flag) setOn(!on);
  }, [on]);
  return <div>{flag || on ? 1 : 0}</div>;
}
