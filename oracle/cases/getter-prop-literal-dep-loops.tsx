// oracle: react=loop
// #321. The child reads a getter of the literal its parent hands it: a new
// object at every read, so at every render of the child alone.
import { useEffect, useState } from "react";

function C({ o }: { o: { p: object } }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}

export function App() {
  return (
    <C
      o={{
        get p() {
          return {};
        },
      }}
    />
  );
}
