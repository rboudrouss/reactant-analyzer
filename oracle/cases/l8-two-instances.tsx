// oracle: react=loop
// oracle: known=fn #175
// L8: the second call site of a child overwrites the first in the results table.
import { useEffect, useState } from "react";

function Counter({ step }: { step: number }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + step);
  }, [n]);
  return <p>{n}</p>;
}

export function App() {
  return (
    <div>
      <Counter step={1} />
      <Counter step={0} />
    </div>
  );
}
