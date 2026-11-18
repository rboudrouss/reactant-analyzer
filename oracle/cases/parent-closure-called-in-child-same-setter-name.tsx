// oracle: react=loop
// oracle: known=fn #326
// A child calls its parent's closure through an object member and declares a
// setter of the same name as the one the closure calls. The closure's `setK`
// is the parent's, so the parent's `k` takes 9, which arms the parent's
// growing effect; the analysis resolves the name in the child's scope and
// writes the child's `k` instead. With distinct names it is claimed.
import { useEffect, useState } from "react";

function Child({ api }: { api: { f: (x: number) => void } }) {
  const [k, setK] = useState(5);
  useEffect(() => {
    api.f(9);
  }, []);
  return <div>{k}</div>;
}

export function App() {
  const [k, setK] = useState(0);
  useEffect(() => {
    if (k > 0) setK(k + 1);
  }, [k]);
  return (
    <div>
      {k}
      <Child api={{ f: (x) => setK(x) }} />
    </div>
  );
}
