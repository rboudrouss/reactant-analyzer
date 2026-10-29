// oracle: react=loop
// #46: a setter returned by a member of a hook's result, `api.get()(x)`.
import { useEffect, useState } from "react";

function useThing() {
  const [v, setV] = useState(0);
  return { v, get: () => setV };
}

export function App() {
  const api = useThing();
  useEffect(() => {
    api.get()(api.v + 1);
  });
  return <div>{api.v}</div>;
}
