import { useState } from 'react';
import { Dialog } from './ui';

// The alias is declared in `ui/dialog.tsx` and re-exported by a barrel: the
// element is the package's through it (#404). An Info.
export function App() {
  const [open, setOpen] = useState(false);
  return <Dialog open={open} onOpenChange={setOpen} />;
}
