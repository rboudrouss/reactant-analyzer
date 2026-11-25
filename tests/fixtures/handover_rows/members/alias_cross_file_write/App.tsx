import { useState } from 'react';
import { Dialog } from './ui';

// The alias is declared in `ui.tsx`; this file writes a program component
// into it, then renders it by member: a write into the package's object
// through the alias (#404). A Warning.
function Local({ fn }: { fn: () => void }) {
  fn();
  return null;
}
(Dialog as any).Close = Local;

export function App() {
  const [n, setN] = useState(0);
  return <Dialog.Close fn={() => setN(n + 1)} />;
}
