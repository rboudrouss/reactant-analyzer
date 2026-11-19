// oracle: react=loop
// #335. A memo that runs once stores a closure calling the setter in a
// module binding; a module object's method the render calls calls it.
import { useMemo, useState } from "react";

let bump = null;
const o = {
  run() {
    if (bump) bump();
    return 1;
  },
};

export function App() {
  const [n, setN] = useState(0);
  useMemo(() => {
    bump = () => setN((c) => c + 1);
  }, []);
  const v = o.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
