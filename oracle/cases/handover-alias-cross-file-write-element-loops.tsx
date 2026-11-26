// @file App.tsx
// oracle: react=loop
// `ui.tsx` aliases a package's object (`export const Dialog = ext`), and this
// file writes a program component into it through the alias, then renders it
// by member. The component calls the prop during its render: React loops. A
// write into a module `const` alias is a write into the package import, in
// any file that imports it (#404), so no element is a package's and the
// hand-over is a Warning (`tp`).
import { useState } from "react";
import { Dialog } from "./ui";

function Local({ fn }: { fn: () => void }) {
  fn();
  return null;
}
(Dialog as any).Close = Local;

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Dialog.Close fn={() => setN(n + 1)} />
    </div>
  );
}

// @file ui.tsx
import ext from "ext";

export const Dialog = ext;
