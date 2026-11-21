// oracle: react=loop
// #64. `forwardRef(Render)` of a named function: React calls it with `ref`
// second and its props without `ref`, so `props.ref` is `undefined` and the
// effect's `!props.ref` test holds although the parent passes a `ref`. A file
// may also render `Render` by its own name, where a React 19 function
// component reads `props.ref`: the analysis keeps both readings of the name.
import { forwardRef, useEffect, useRef, useState } from "react";

function Render(props: any, ref: any) {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!props.ref) setN(n + 1);
  });
  return <div>{n}</div>;
}

// The runner names the component by its `displayName`: the analysis names the
// loop after the function it analyses.
const Field = forwardRef<HTMLDivElement, {}>(Render);
Field.displayName = "Render";

export function App() {
  const r = useRef<HTMLDivElement>(null);
  return <Field ref={r} />;
}
