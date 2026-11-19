// oracle: react=loop
// #309: the parent passes `undefined`, so the default is a new array at
// every render of the child, and every write stores a new one.
import { useEffect, useState } from "react";

function C0({ p1 = [] }) {
  const [s1, setS1] = useState<unknown>(null);
  useEffect(() => {
    setS1(p1);
  }, [s1]);
  return <div>{String(s1)}</div>;
}

export function App() {
  return (
    <div>
      <C0 p1={undefined} />
    </div>
  );
}
