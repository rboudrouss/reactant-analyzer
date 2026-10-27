// oracle: react=loop
// A deps-list element is evaluated during render at every render (#193): the
// call in it is a render-phase write, the memo body is not.
import { useMemo, useState } from "react";

export function App() {
  const [a, setA] = useState(0);
  const v = useMemo(() => 2, [setA(a + 1)]);
  return <div>{a}{v}</div>;
}
