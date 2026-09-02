import { useState } from "react";

// `renderRows` captures `onPick` — a setter the parent passed down — but only
// puts it inside a JSX handler. Calling it during render writes nothing, so the
// row may not be certified even though the call dominates every exit.
//
// The *must* side of the same distinction — a closure whose own body calls what
// it captured — is `Section9_Child` in tests/fixtures/inter_component.tsx.
export function List({ onPick }: { onPick: (id: string) => void }) {
  const renderRows = (ids: string[]) =>
    ids.map((id) => (
      <li key={id} onClick={() => onPick(id)}>
        {id}
      </li>
    ));
  return <ul>{renderRows(["a", "b"])}</ul>;
}

export function Parent() {
  const [picked, setPicked] = useState("");
  return (
    <div>
      <List onPick={setPicked} />
      {picked}
    </div>
  );
}
