// oracle: react=loop
// The parent's loop is armed only by the write its closure makes when the
// child hands it to code the analysis does not see, once, at mount: the
// parent's `n` has to take that write for `n > 0` to be possible. The child
// declares a setter of the same name, which the closure does not see.
import { useEffect, useState } from "react";
import { runNow } from "ext";

function Child({ f }: { f: () => void }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(f);
  }, []);
  return <div>{n}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (n > 0) setN(n + 1);
  }, [n]);
  return (
    <div>
      {n}
      <Child f={() => setN(1)} />
    </div>
  );
}
