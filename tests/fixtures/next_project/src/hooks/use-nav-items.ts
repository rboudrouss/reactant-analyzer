import { useState, useMemo } from "react";

export function useNavItems() {
  const [items] = useState(["home"]);
  return useMemo(() => items, [items]);
}
