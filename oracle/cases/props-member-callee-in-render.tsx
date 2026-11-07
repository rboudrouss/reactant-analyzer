// oracle: react=loop
// #243 in render: `props.onChange(1)` runs the parent's closure while the
// child renders, a write into the parent during the child's render.
import { useState } from "react";

function Child(props: { onChange: (d: number) => void }) {
  props.onChange(1);
  return <div />;
}

export function App() {
  const [v, setV] = useState(0);
  return <Child onChange={(d) => setV((x) => x + d)} />;
}
