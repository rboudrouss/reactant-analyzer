// oracle: react=loop
// #64. `forwardRef((props, ref) => …)` renders the arrow with the element's
// `ref`; its effect writes a new value after every commit. A `displayName`
// written after the definition changes nothing the analysis reads.
import { forwardRef, useEffect, useState } from "react";

const Field = forwardRef<HTMLInputElement, { label: string }>((props, ref) => {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return <input ref={ref} aria-label={props.label} value={n} readOnly />;
});
Field.displayName = "Field";

export function App() {
  return <Field label="f" />;
}
