import { useThing } from "./useThing";
import { useLingui } from "@lingui/react";

// This component binds `t` too — a hook value, a different binding entirely.
// Inlining `useThing` must not let this `t` capture the module import the
// callee reads.
export function Captures({ x }: { x: number }) {
  const { t } = useLingui();
  const cb = useThing(x);
  return <button onClick={cb}>{t`hi`}</button>;
}
