// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents tp
// W6's u1n: the package calls its `fn` prop after every commit, and the prop
// clears the draft the effect fills; the effect re-runs on the cleared
// draft. React loops. The effect's own fresh write under a dep that moves
// with its slot is a cycle the analysis claims without the hand-over, so the
// loop stays a Warning under `packagePropsRunOnEvents` too (`tp` both ways).
import { useEffect, useState } from "react";
import { EffectCalls } from "ext";

function Editor({ value }: { value: string[] }) {
  const [draft, setDraft] = useState<string[]>([]);
  useEffect(() => {
    setDraft(value.map((v) => v.trim()));
  }, [value, draft.length === 0]);
  return <EffectCalls fn={() => setDraft([])} />;
}

const VALUE = ["a ", "b"];
export function App() {
  return <Editor value={VALUE} />;
}
