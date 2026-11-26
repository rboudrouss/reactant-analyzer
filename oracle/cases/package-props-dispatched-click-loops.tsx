// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents tp
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers fn
// The package calls its `fn` prop from a click only, and the program clicks
// the package's button itself, from an effect after every commit: the event
// is the program's, so the handler runs at the effect's time. By default the
// prop is the Info hand-over (`disclosed`). Under `packagePropsRunOnEvents`
// it is a listener a dispatch of any event may reach (#407), as a host
// element's `on*` prop: the effect's click runs it, and the loop is claimed.
// With `programEventsMissPackageHandlers` too, no event the program fires
// reaches it, against this program: `fn`, the second assumption's price.
import { useEffect, useRef, useState } from "react";
import { ClickCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    box.current?.querySelector("button")?.click();
  });
  return (
    <div ref={box}>
      {n}
      <ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
