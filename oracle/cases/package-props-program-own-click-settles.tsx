// oracle: react=converges
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers tn
// The program clicks its own button, outside any package component, from an
// effect after every commit; a package component beside it calls its prop
// from a click on its own button only. The program's click never reaches the
// package: React settles. Under `packagePropsRunOnEvents` alone the prop is a
// listener every dispatch of the program may reach (the analysis does not
// place the package's DOM); with `programEventsMissPackageHandlers` too, no
// dispatch reaches it.
import { useEffect, useRef, useState } from "react";
import { ClickCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const own = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    own.current?.click();
  });
  return (
    <div>
      <button ref={own}>{n}</button>
      <ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
