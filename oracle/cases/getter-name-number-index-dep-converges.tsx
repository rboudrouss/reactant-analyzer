// oracle: react=converges
// #336. The program's only accessor is named `p`; `rows[i % 2]` reads the
// key of a number, which no accessor named `p` may be, so the element is the
// module literal's, the same object at every render: the effect runs once.
import { useEffect, useState } from "react";

const g = {
  get p() {
    return 1;
  },
};
const rows = [{ id: 1 }, { id: 2 }];

export function App() {
  const [i] = useState(0);
  const [n, setN] = useState(0);
  const row = rows[i % 2];
  useEffect(() => {
    setN((c) => c + 1);
  }, [row]);
  return (
    <div>
      {n}
      {g.p}
    </div>
  );
}
