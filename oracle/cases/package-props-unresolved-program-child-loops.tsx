// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents tp
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers tp
// The child is program code the analysis does not resolve (an element value
// read off an array), which calls the prop from an effect after every commit.
// It is no package's: `packagePropsRunOnEvents` does not touch it, and the
// hand-over stays a Warning (`tp` both ways).
import { useEffect, useState } from "react";

function Effecty({ fn }: { fn: () => void }) {
  useEffect(() => {
    fn();
  });
  return null;
}

const KINDS = [Effecty];

export function App() {
  const [n, setN] = useState(0);
  const C = KINDS[0];
  return (
    <div>
      {n}
      <C fn={() => setN(n + 1)} />
    </div>
  );
}
