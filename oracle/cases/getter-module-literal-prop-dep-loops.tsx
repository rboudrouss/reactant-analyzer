// oracle: react=loop
// #334. A module-level literal handed down as a prop: the child's `o.p`
// runs the getter, a new object at every render of the child.
import { useEffect, useState } from "react";

function C({ o }: { o: { p: object } }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}

const shared = {
  get p() {
    return {};
  },
};

export function App() {
  return <C o={shared} />;
}
