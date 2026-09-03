import { useThing } from "../shared/useThing";

// A plain relative import that resolves to a real file outside the directory
// the run was pointed at (#9): the resolver knows exactly where `useThing`
// lives and discovery never walks there.
export function App() {
  const thing = useThing();
  return <div>{thing}</div>;
}
