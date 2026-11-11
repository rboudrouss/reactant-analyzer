// oracle: react=loop
// #243 through an alias of the props object: `p` points where `props` does,
// so `p.f` is the parent's closure too.
import { useEffect, useState } from "react";

function Child(props: { f: () => void }) {
  const p = props;
  useEffect(() => {
    p.f();
  });
  return <div />;
}

export function App() {
  const [v, setV] = useState(0);
  return <Child f={() => setV((x) => x + 1)} />;
}
