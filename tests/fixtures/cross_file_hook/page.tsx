// Component imports a custom hook from a sibling file.
// `useData`'s definition lives in ./hooks/useData.ts; the analyzer must
// resolve the relative specifier, populate HookEntry::Custom::resolved_file,
// and look the hook up by (file, name) — not just by name.

import { useData } from "./hooks/useData";

function Page() {
  const data = useData(0);
  return <div>{data}</div>;
}
