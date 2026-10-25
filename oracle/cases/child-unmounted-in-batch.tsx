// oracle: react=converges
// Found by `oracle fuzz --seed 3` (program 1366, shrunk). `Child` recreates the
// object state its effect depends on, but the parent's mount effect moves the
// guard `Child` is rendered under in the same batch: the re-render that would
// run the effect again unmounts it instead. A loop needs its component still
// mounted, so every parent must keep it mounted (#162's proof).
import { useEffect, useState } from "react";

function Child() {
  const [s, setS] = useState([]);
  useEffect(() => {
    setS([...s, 1]);
  }, [s]);
  return <div />;
}

export function App() {
  const [shown, setShown] = useState(true);
  useEffect(() => {
    setShown(false);
  }, []);
  return <div>{shown && <Child />}</div>;
}
