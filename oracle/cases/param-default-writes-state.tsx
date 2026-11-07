// oracle: react=loop
// A parameter's default runs when the argument is omitted: `go()` evaluates
// `setS({})` on every run of the effect, a new object into the slot its deps
// read. The lowering dropped the default, and with it the write.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({});
  useEffect(() => {
    const go = (v = setS({})) => v;
    go();
  }, [s]);
  return <div />;
}
