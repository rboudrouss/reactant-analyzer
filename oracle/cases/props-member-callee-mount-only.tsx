// oracle: react=converges
// #243, the precision side: the same `props.f()` in a mount-only effect
// writes the parent's slot once, and the child's effect never runs again.
import { useEffect, useState } from "react";

function Child(props: { f: () => void }) {
  useEffect(() => {
    props.f();
  }, []);
  return <div />;
}

export function App() {
  const [v, setV] = useState(0);
  return <Child f={() => setV((x) => x + 1)} />;
}
