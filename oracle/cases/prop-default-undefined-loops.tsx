// oracle: react=loop
// #309: an explicit `undefined` takes the default too: a new array at
// every render of the child.
import { useEffect, useState } from "react";

function C0({ p1 = [] }) {
  const [s1, setS1] = useState(0);
  useEffect(() => {
    setS1({ v: 0 });
  }, [p1]);
  return <div>{String(s1)}</div>;
}

export function App() {
  return (
    <div>
      <C0 p1={undefined} />
    </div>
  );
}
