// oracle: react=loop
// #412. A fresh ref callback handed to a `forwardRef` child, which puts it on
// its host element: React attaches it there at every commit of the parent.
import { forwardRef, useState } from "react";

const Box = forwardRef<HTMLDivElement, { label: string }>(function Box(p, ref) {
  return <div ref={ref}>{p.label}</div>;
});
Box.displayName = "Box";

export function App() {
  const [w, setW] = useState<unknown>(0);
  return (
    <Box
      label={String(!!w)}
      ref={(el) => {
        setW({ el });
      }}
    />
  );
}
