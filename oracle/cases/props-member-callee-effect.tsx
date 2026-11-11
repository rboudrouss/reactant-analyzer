// oracle: react=loop
// #243: the destructured `({ f }) => … f()` closes the loop, the same call
// through the props object was silent. `props.f` is the member of the props
// literal the parent passed: its closure writes the parent's slot on every
// effect run, and every write re-renders the child.
import { useEffect, useState } from "react";

function Child(props: { f: () => void }) {
  useEffect(() => {
    props.f();
  });
  return <div />;
}

export function App() {
  const [v, setV] = useState(0);
  return <Child f={() => setV((x) => x + 1)} />;
}
