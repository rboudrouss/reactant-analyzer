// oracle: react=converges
// #392. A polymorphic `as` prop: the parent passes a host tag, so `Box`'s
// `<Comp/>` is a host button, whose `onClick` is a listener: nothing writes
// the state while rendering.
import { useState } from "react";

function Box({ as: Comp = "div", ...rest }: { as?: any; onClick: () => void; children?: any }) {
  return <Comp {...rest} />;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <Box as="button" onClick={() => setS(s + 1)}>
      {s}
    </Box>
  );
}
