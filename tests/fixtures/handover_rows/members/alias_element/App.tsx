import { useState } from 'react';
import * as DialogPrimitive from 'ext';

// A module `const` aliasing a package's member is that package's element,
// as `<DialogPrimitive.Root>` written in place is (#404): an Info. So is a
// member element off the alias.
const Dialog = DialogPrimitive.Root;
const DialogClose = (Dialog as any).Close;

export function App() {
  const [open, setOpen] = useState(false);
  const [n, setN] = useState(0);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogClose onClick={() => setN(n + 1)} />
    </Dialog>
  );
}
