// Imports the buggy hook through the tsconfig alias `@/*` → `./src/*`.
// The infinite loop in useData must surface on `App`: this proves alias
// resolution feeds the cross-file hook inlining pipeline.

import { useData } from "@/hooks/useData";

function App() {
  const data = useData(0);
  return <div>{data}</div>;
}
