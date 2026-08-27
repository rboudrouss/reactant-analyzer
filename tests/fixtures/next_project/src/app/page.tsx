// No "use client": the App Router renders this on the server, so the useState
// below cannot run. `server-component-hook` must fire here.
import { useState } from "react";
import { Counter } from "@/components/counter";

export default function HomePage() {
  const [open, setOpen] = useState(false);
  return (
    <main onClick={() => setOpen(true)}>
      <Counter start={3} />
      {open ? "open" : "closed"}
    </main>
  );
}
