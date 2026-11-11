// oracle: react=converges
// #289, the closure an unresolved child never calls: the analysis cannot see
// that it is never called, so the write it would make is possible, never
// certain. At most a Warning; an Error here would be error-unsound.
import { useEffect, useState } from "react";

function Kid(_props: { api: { run: () => void } }) {
  return null;
}
const kids = { Kid };

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    if (v > 0) setV(v + 1);
  }, [v]);
  return (
    <kids.Kid
      api={{
        run: () => {
          const s = setV;
          s(1);
        },
      }}
    />
  );
}
