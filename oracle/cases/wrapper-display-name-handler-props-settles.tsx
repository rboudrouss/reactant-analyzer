// @file App.tsx
// oracle: react=converges
// #64. A `forwardRef` input and a `memo` of a name, each with a `displayName`
// written after the definition: the parent hands them closures that set its
// state, a callback `ref` among them, which the children run only from a DOM
// event or as the host input's ref (one commit, the same node). Seen through
// the wrappers, nothing runs at render; the member writes on the wrapped
// components escape nothing.
import { useCallback, useState } from "react";
import { Input, Label } from "./ui";

export function App() {
  const [el, setEl] = useState<HTMLInputElement | null>(null);
  const [v, setV] = useState("");
  const ref = useCallback((node: HTMLInputElement | null) => setEl(node), []);
  return (
    <div>
      {String(el !== null)}
      <Label onPick={() => setV("label")} />
      <Input ref={ref} value={v} onChange={(e) => setV(e.target.value)} />
    </div>
  );
}

// @file ui.tsx
import * as React from "react";

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, ...props }, ref) => <input className={className} ref={ref} {...props} />,
);
Input.displayName = "Input";

function LabelImpl({ onPick }: { onPick: () => void }) {
  return <label onClick={() => onPick()}>label</label>;
}
export const Label = React.memo(LabelImpl);
Label.displayName = "Label";
