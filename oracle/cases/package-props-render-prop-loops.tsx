// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents disclosed
// A package component that calls its `children` during its render (a render
// prop, as `<AutoSizer>{(size) => …}</AutoSizer>` does): the render prop
// writes the parent's state at every render of the package. `children` is
// not covered by `packagePropsRunOnEvents` (its result is what the element
// renders): the Info hand-over stays (`disclosed` both ways).
import { useState } from "react";
import { RenderProp } from "ext";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <RenderProp>
        {() => {
          setN(n + 1);
          return null;
        }}
      </RenderProp>
    </div>
  );
}
