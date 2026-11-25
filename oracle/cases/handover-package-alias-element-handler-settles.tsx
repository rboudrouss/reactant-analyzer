// @file App.tsx
// oracle: react=converges
// A program file binds a package's component to its own name
// (`export const Dialog = DialogPrimitive.Root`) and another file renders it:
// the element instantiates the package's code exactly as `<ext.ClickCalls>`
// written in place does (#404). It calls the setter from a button's `onClick`
// only: React settles, and the hand-over row is an Info (`tn`); before #404 the
// element was a program binding the analysis did not resolve, a Warning (`fp`).
import { useState } from "react";
import { Clicky } from "./ui";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Clicky fn={() => setN(n + 1)} />
    </div>
  );
}

// @file ui.tsx
import ext from "ext";

export const Clicky = ext.ClickCalls;
