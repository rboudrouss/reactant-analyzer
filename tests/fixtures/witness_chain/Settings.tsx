import { useState } from "react";
import { loadPrefs } from "./prefs";

export function Settings() {
  const [prefs, setPrefs] = useState(loadPrefs("theme"));
  return <div>{prefs}</div>;
}
