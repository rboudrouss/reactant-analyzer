// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents tp
// oracle: assume=packagePropsRunOnEvents,programEventsMissPackageHandlers fn
// A program button rendered inside a package component whose handler listens
// on an ancestor (its wrapper's `onClick`): the program clicks its own button
// from an effect after every commit, the click bubbles to the package's node,
// and the package calls the prop. React loops. By default the prop is the Info
// hand-over (`disclosed`); under `packagePropsRunOnEvents` it is a listener
// the effect's click reaches (`tp`); under both assumptions no event the
// program fires reaches it, against this program: `fn`, the second
// assumption's price (ADR-048).
import { useEffect, useRef, useState } from "react";
import { ClickAncestor } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    button.current?.click();
  });
  return (
    <ClickAncestor fn={() => setN(n + 1)}>
      <button ref={button}>{n}</button>
    </ClickAncestor>
  );
}
