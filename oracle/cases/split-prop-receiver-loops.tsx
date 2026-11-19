// oracle: react=loop
// #312: `x.split(",")` off a prop is a new array at every render of the
// child, so the effect loops. The parent passes what a package returns, of a
// kind the analysis does not know: the result may be new, a possible loop.
import { useEffect, useState } from "react";
import { format } from "ext";

function C({ x }: { x: string }) {
  const [s, setS] = useState(0);
  const o = x.split(",");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function App() {
  return <C x={format("a,b")} />;
}
