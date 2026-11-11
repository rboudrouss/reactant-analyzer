// oracle: react=loop
// #265: `children` is a new element whenever the parent renders, here
// because the child's effect re-renders the parent.
import { useState, useEffect } from "react";

function C({ children, bump }) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
    bump();
  }, [children]);
  return <div />;
}

export function App() {
  const [k, setK] = useState(0);
  return (
    <C bump={() => setK(k + 1)}>
      <b />
    </C>
  );
}
