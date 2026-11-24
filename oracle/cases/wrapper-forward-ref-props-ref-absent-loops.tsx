// oracle: react=loop
// #64. React removes `ref` from the props it hands a `forwardRef` render
// (react-dom `updateForwardRef`) and passes it second: `props.ref` is
// `undefined` inside the render, whatever the parent passes. The effect's
// `!props.ref` test holds, so it writes after every commit although the
// parent gave a `ref`.
import { forwardRef, useEffect, useRef, useState } from "react";

const Field = forwardRef<HTMLDivElement, {}>(function Field(props: any, ref) {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!props.ref) setN(n + 1);
  });
  return <div ref={ref}>{n}</div>;
});
Field.displayName = "Field";

export function App() {
  const r = useRef<HTMLDivElement>(null);
  return <Field ref={r} />;
}
