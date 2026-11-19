// oracle: react=loop
// #313, the looping twin: the same child mounted through a value the tree
// cannot place, with nothing unmounting it, stores a new object after every
// render.
import { useEffect, useState } from "react";

function C() {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
  });
  return <div>{Object.keys(s).length}</div>;
}

function D() {
  return <span />;
}

export function App() {
  const [n] = useState(0);
  const Comp = n >= 0 ? C : D;
  return <Comp />;
}
