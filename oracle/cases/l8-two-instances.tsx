// oracle: react=loop
// L8 (#175): the second call site of a child used to overwrite the first in
// the results table. The child is analysed under the join of both sites'
// props: `step` is 0 or 1, and the first instance's loop is seen.
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
