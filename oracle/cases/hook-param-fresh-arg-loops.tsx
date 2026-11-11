// oracle: react=loop
// #265: a custom hook's parameter is the call's argument, here a new object
// at every render of the caller.
import { useState, useEffect } from "react";

function useT(dep) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
  }, [dep]);
  return s;
}

export function App() {
  useT({});
  return <div />;
}
