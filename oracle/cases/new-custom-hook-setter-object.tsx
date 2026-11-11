// oracle: react=loop
// #185, the object shape: `return { v, setV }` destructured as
// `const { v, setV } = useSlot()`. The setter reached the component as a
// member read of the spliced literal, which no setter consumer followed.
import { useEffect, useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return { v, setV };
}

export function App() {
  const { v, setV } = useSlot();
  useEffect(() => {
    setV(v + 1);
  }, [v]);
  return <div>{v}</div>;
}
