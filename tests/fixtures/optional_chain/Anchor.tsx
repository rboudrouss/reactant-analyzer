import { useRef, useState } from "react";
import { TextInput, useHotkeys } from "somewhere";

// One setter handed to callees with no timing summary at two sites: the row
// names the first one in the source, whatever a `?.` does to the blocks.
export function HandlerSetters({ onBlur }: { onBlur?: (e: unknown) => void }) {
  const [isFocused, setIsFocused] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const handleBlur = (e: unknown) => {
    onBlur?.(e);
    setIsFocused(false);
  };
  const handleEscape = () => {
    if (!isFocused) return;
    inputRef.current?.blur();
    setIsFocused(false);
  };
  useHotkeys("esc", handleEscape);
  return <TextInput ref={inputRef} onBlur={handleBlur} />;
}
