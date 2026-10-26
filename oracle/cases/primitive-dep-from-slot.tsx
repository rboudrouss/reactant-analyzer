// oracle: react=loop
// #203. The dep `!on` is a boolean computed from the slot `on`: it carries no
// version label, but it changes whenever `on` does, and the effect flips `on`
// at every run.
import { useEffect, useState } from "react";

export function App() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    setOn(!on);
  }, [!on]);
  return <div>{String(on)}</div>;
}
