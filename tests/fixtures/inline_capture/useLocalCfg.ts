import { useEffect } from "react";

// `cfg` is a callee LOCAL and fresh every render — a real always-unstable-deps
// finding that must survive, even though a caller below binds the same name.
export const useLocalCfg = () => {
  const cfg = { a: 1 };
  useEffect(() => {
    console.log(cfg);
  }, [cfg]);
};
