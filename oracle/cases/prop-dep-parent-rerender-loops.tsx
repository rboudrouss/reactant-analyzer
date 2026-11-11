// oracle: react=loop
// #265: a prop dep moves when the parent re-renders the child with a new
// value, here because the child's effect re-renders the parent.
import { useState, useEffect } from "react";

function C({ dep, bump }) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
    bump();
  }, [dep]);
  return <div />;
}

export function App() {
  const [k, setK] = useState(0);
  return <C dep={{}} bump={() => setK(k + 1)} />;
}
