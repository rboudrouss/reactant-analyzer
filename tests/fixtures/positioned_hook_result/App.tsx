import { useMemo } from "react";
import { useEvents } from "./use-events";

export function App({ handler }: { handler: () => void }) {
  const { events } = useEvents(handler);
  const props = useMemo(() => ({ ...events }), []);
  return <button {...props} />;
}
