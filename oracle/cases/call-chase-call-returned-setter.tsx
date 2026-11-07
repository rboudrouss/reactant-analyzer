// oracle: react=loop
// #46: a setter called through a call-returned function, `get()(x)`.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  useEffect(() => {
    get()(v + 1);
  });
  return <div>{v}</div>;
}
