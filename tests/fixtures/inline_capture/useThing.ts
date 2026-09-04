import { useCallback } from "react";
// `t` is a MODULE binding here: constant for the life of the module, so it can
// never belong in a deps array.
import { t } from "@lingui/core/macro";

export const useThing = (x: number) => {
  return useCallback(() => {
    console.log(t`boom`, x);
  }, [x]);
};
