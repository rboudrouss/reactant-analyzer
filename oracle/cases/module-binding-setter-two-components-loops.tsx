// oracle: react=loop
// #335. The binding is shared by two components: the parent stores its
// setter there, and the child's render calls a module object's method that
// calls it, which renders the parent, and the child again.
import { useState } from "react";

let setter = null;
const o = {
  run() {
    if (setter) setter((c) => c + 1);
    return 1;
  },
};

function Child() {
  const v = o.run();
  return <span>{v}</span>;
}

export function App() {
  const [n, setN] = useState(0);
  setter = setN;
  return (
    <div>
      {n}
      <Child />
    </div>
  );
}
