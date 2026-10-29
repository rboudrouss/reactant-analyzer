// oracle: react=converges
// #213. `Mid` renders `Child` on every path, but `App`'s mount effect moves the
// guard `Mid` is rendered under, in the batch that should re-run `Child`'s
// effect: `Child` is unmounted with its parent. Staying mounted is transitive,
// so the loop in `Child` is no certain Error.
import { useEffect, useState } from "react";

function Child() {
  const [s, setS] = useState([]);
  useEffect(() => {
    setS([...s, 1]);
  }, [s]);
  return <div />;
}

function Mid() {
  return <Child />;
}

export function App() {
  const [shown, setShown] = useState(true);
  useEffect(() => {
    setShown(false);
  }, []);
  return <div>{shown && <Mid />}</div>;
}
