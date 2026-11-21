// oracle: react=loop
// #64, the other side of `wrapper-forward-ref-props-ref-absent-settles`: the
// test is `!props.ref`, which holds in a `forwardRef` render (`props.ref` is
// `undefined`, the element's `ref` is the second argument), so the render
// calls the parent's setter after every render of it.
import { forwardRef, useRef, useState } from "react";

const Field = forwardRef<HTMLDivElement, { bump: () => void }>(function Field(props: any, ref) {
  if (!props.ref) props.bump();
  return <div ref={ref} />;
});
Field.displayName = "Field";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<HTMLDivElement>(null);
  return <Field ref={r} bump={() => setN(n + 1)} />;
}
