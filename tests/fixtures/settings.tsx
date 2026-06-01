import React, { useState, useEffect, useCallback } from "react";

// ── ThemeToggle ──────────────────────────────────────────────────────────────
//
// Bug: setTheme("dark") in effect where theme is already set to "dark"
// (after first render the state settles to "dark" → Stable → redundant update).

function ThemeToggle() {
  const [theme, setTheme] = useState("light");
  const [fontSize, setFontSize] = useState(14);

  // After the first run, theme == "dark" (Stable literal) and state is Stable.
  // ⚠ redundant-set-state: setTheme("dark") when state is already Stable.
  useEffect(() => {
    setTheme("dark");
  }, []);

  const increaseFontSize = useCallback(() => {
    setFontSize((n) => n + 1);
  }, []);

  return (
    <div data-theme={theme} style={{ fontSize }}>
      <button onClick={() => setTheme("light")}>Light</button>
      <button onClick={() => setTheme("dark")}>Dark</button>
      <button onClick={increaseFontSize}>A+</button>
    </div>
  );
}

// ── LanguageSelector ──────────────────────────────────────────────────────────
//
// Clean: reads from localStorage (modelled as a Call → Unknown), correct deps.

function LanguageSelector() {
  const [lang, setLang] = useState("en");

  useEffect(() => {
    const stored = localStorage.getItem("lang");
    if (stored) {
      setLang(stored);
    }
  }, []);

  useEffect(() => {
    localStorage.setItem("lang", lang);
  }, [lang]);

  return (
    <select value={lang} onChange={(e) => setLang(e.target.value)}>
      <option value="en">English</option>
      <option value="fr">Français</option>
      <option value="es">Español</option>
    </select>
  );
}

// ── NotificationPrefs ─────────────────────────────────────────────────────────
//
// Clean: multiple boolean states, all used, effects correct.

function NotificationPrefs() {
  const [email, setEmail] = useState(true);
  const [push, setPush] = useState(false);
  const [sms, setSms] = useState(false);

  // Reads all three states — no dead state.
  useEffect(() => {
    fetch("/api/prefs", {
      method: "POST",
      body: JSON.stringify({ email, push, sms }),
    });
  }, [email, push, sms]);

  return (
    <div>
      <label>
        <input
          type="checkbox"
          checked={email}
          onChange={(e) => setEmail(e.target.checked)}
        />
        Email
      </label>
      <label>
        <input
          type="checkbox"
          checked={push}
          onChange={(e) => setPush(e.target.checked)}
        />
        Push
      </label>
      <label>
        <input
          type="checkbox"
          checked={sms}
          onChange={(e) => setSms(e.target.checked)}
        />
        SMS
      </label>
    </div>
  );
}
