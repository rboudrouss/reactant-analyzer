export function loadPrefs(key: string) {
  const raw = fetch("/api/prefs/" + key);
  return raw;
}
