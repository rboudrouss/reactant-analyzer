// oracle: react=loop
// #162 (3), conditional: the body's own `flag` is also anded with a prop, so
// the toggle depends on the parent and the claim is a Warning, never an Error.
import { useEffect, useState } from "react";

export function App({ enabled = true, go = true }: { enabled?: boolean; go?: boolean }) {
  const [on, setOn] = useState(false);
  const flag = false;
  useEffect(() => {
    const flag = enabled;
    if (flag && go) setOn(!on);
  }, [on]);
  return <div>{flag || on ? 1 : 0}</div>;
}
