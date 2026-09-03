import { useMystery } from "some-unknown-package";

// `extract_hooks` rewrites a block's statements and never looks at its
// terminator, so a hook reached only through a `return` or a branch condition
// produced no HookEntry at all (#4). The component reported zero hooks, no
// `analysis-limit` fired, and every rule passed it in silence — the worst
// direction, because the assurance channel then *claimed* what it had not
// checked.

// Return position: a custom hook whose whole body is `return <hook call>`.
export function useDirect(x: number) {
  return useMystery(x);
}

export function ViaReturn() {
  const v = useDirect(1);
  return <div>{v}</div>;
}

// Branch condition.
export function InCondition({ flag }: { flag: boolean }) {
  if (useMystery(flag)) {
    return <div>a</div>;
  }
  return <div>b</div>;
}

// The control: the same hook in statement position always worked, and the
// three must agree that the analysis was truncated.
export function InStatement({ flag }: { flag: boolean }) {
  const v = useMystery(flag);
  return <div>{v}</div>;
}
