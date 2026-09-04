import { useCallback } from "react";
import { useLingui } from "@lingui/react";

// The caller's OWN `t`, read in the caller's OWN callback and absent from its
// deps. This one is a real finding and must survive: the fix isolates the
// callee's names, it does not stop analysing the caller's.
export function OwnBinding({ x }: { x: number }) {
  const { t } = useLingui();
  const cb = useCallback(() => {
    console.log(t`hi`, x);
  }, [x]);
  return <button onClick={cb} />;
}
