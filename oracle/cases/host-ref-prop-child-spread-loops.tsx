// oracle: react=loop
// #412. React 19: `ref` is a prop of a function component; a child that
// spreads its props on a host element gets the parent's fresh callback
// attached at every commit.
import { useState } from "react";

function Field(props: { ref?: (el: HTMLInputElement | null) => void; name: string }) {
  return <input {...props} />;
}

export function App() {
  const [w, setW] = useState<unknown>(0);
  return (
    <Field
      name={String(!!w)}
      ref={(el) => {
        setW({ el });
      }}
    />
  );
}
