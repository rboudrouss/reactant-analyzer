// oracle: react=loop
// ADR-017 (`ObjChurn`): no value grows, the loop is carried by identity alone.
import { useEffect, useState } from "react";

export function App() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [obj]);
  return <div>{obj.a}</div>;
}
