// Page imports a utility from a neighboring file.
// `ImportResolver` resolves ./lib/helpers → /…/lib/helpers.ts;
// `expand_utility_calls` splices `bump`'s body into the effect.

import { bump } from "./lib/helpers";

function Page() {
  const [c, setC] = useState(0);
  useEffect(() => {
    bump(setC);
  }, []);
  return <div>{c}</div>;
}
