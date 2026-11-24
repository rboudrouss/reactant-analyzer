// oracle: react=loop
// #344. Both closures `make` may return loop, through a ternary: whichever
// the program picks at module time, its effect moves the slot it depends on.
import { useState, useEffect } from "react";

const DEBUG = false;

function make(debug) {
  const plain = () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + 1);
    }, [n]);
    return n;
  };
  const traced = () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      console.log(n);
      setN(n + 2);
    }, [n]);
    return n;
  };
  return debug ? traced : plain;
}

const useX = make(DEBUG);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
