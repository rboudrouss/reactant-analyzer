// oracle: react=converges
// oracle: known=error-unsound #188
// Found by `oracle fuzz --seed 1` (program 288, shrunk). Same as
// new-last-write-restores.tsx with the two writes in two effects of one commit:
// the second effect's write is the one the queue ends on.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS([]);
  });
  useEffect(() => {
    setS(0);
  });
  return <div />;
}
