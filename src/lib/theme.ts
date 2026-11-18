import { readPreference, writePreference } from "@/lib/persisted-preference";

export type ThemePreference = "system" | "light" | "dark";

function isThemePreference(value: string | null): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

const THEME_PREFERENCE = {
  key: "workbuddy-shift.theme",
  fallback: "system" as ThemePreference,
  parse: (raw: string): ThemePreference | null => (isThemePreference(raw) ? raw : null),
  serialize: (value: ThemePreference) => value,
  eventName: "workbuddy-shift:theme-changed",
} as const;

export function getThemePreference(): ThemePreference {
  return readPreference(THEME_PREFERENCE);
}

export function applyTheme(preference: ThemePreference) {
  const dark =
    preference === "dark" ||
    (preference === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export function setThemePreference(preference: ThemePreference) {
  writePreference(THEME_PREFERENCE, preference);
  applyTheme(preference);
}

export function watchSystemTheme() {
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const onChange = () => {
    if (getThemePreference() === "system") applyTheme("system");
  };

  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}
