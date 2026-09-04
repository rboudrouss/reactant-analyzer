import { useLocalCfg } from "./useLocalCfg";
import { cfg } from "@some/pkg";

// Same name as the callee's local, bound in the caller. The callee's local must
// still shadow it, so the finding inside `useLocalCfg` survives.
export function ShadowGuard() {
  useLocalCfg();
  return <div>{String(cfg)}</div>;
}
