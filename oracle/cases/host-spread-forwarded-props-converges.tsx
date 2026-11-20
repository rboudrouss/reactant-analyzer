// oracle: react=converges
// #371: `Input` spreads its props onto a host element, whose members React DOM
// converts. A forwarding wrapper the analysis never renders spreads props it
// does not place into `Input`, so one of the objects converted may inherit a
// `toString` the analysis does not see: that object is handed over, not the
// handler `App` passes, which React only calls on an event.
import { useState } from "react";

function Input({ className, ...props }: { className?: string; value?: string; onChange?: (e: { target: { value: string } }) => void }) {
  return <input className={className} {...props} />;
}

export function SidebarInput({ className, ...props }: { className?: string }) {
  return <Input className={className} {...props} />;
}

export function App() {
  const [value, setValue] = useState("");
  return (
    <div>
      <Input value={value} onChange={(e) => setValue(e.target.value)} />
    </div>
  );
}
