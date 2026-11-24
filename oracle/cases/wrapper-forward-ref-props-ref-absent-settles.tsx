// oracle: react=converges
// #64, the neighbour of `wrapper-forward-ref-props-ref-absent-loops`:
// `props.ref` is `undefined` in a `forwardRef` render, so the render never
// calls the setter the parent hands it, although the parent passes a `ref`.
// Reading the parent's `ref` as `props.ref` ran a render-phase write that does
// not exist.
import { forwardRef, useRef, useState } from "react";

const Field = forwardRef<HTMLDivElement, { bump: () => void }>(function Field(props: any, ref) {
  if (props.ref) props.bump();
  return <div ref={ref} />;
});
Field.displayName = "Field";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<HTMLDivElement>(null);
  return <Field ref={r} bump={() => setN(n + 1)} />;
}
