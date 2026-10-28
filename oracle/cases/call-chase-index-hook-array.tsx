// oracle: react=loop
// #46: a setter called through an index into an array a custom hook returns.
import { useEffect, useState } from "react";

function useSetters() {
  const [v, setV] = useState(0);
  return [v, [setV]] as const;
}

export function App() {
  const [v, fns] = useSetters();
  useEffect(() => {
    fns[0](v + 1);
  });
  return <div>{v}</div>;
}
