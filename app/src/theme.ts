export type ThemePref = "system" | "dark" | "light";

const KEY = "roomy-theme";
const media = window.matchMedia("(prefers-color-scheme: light)");

export function getThemePref(): ThemePref {
  const v = localStorage.getItem(KEY);
  // Light first (brand v2); "system" and "dark" are opt-in.
  return v === "dark" || v === "system" ? v : "light";
}

function apply(pref: ThemePref) {
  const resolved = pref === "system" ? (media.matches ? "light" : "dark") : pref;
  document.documentElement.dataset.theme = resolved;
  window.dispatchEvent(new CustomEvent("roomy-theme", { detail: resolved }));
}

export function setThemePref(pref: ThemePref) {
  localStorage.setItem(KEY, pref);
  apply(pref);
}

export function initTheme() {
  apply(getThemePref());
  media.addEventListener("change", () => {
    if (getThemePref() === "system") apply("system");
  });
}

export function currentTheme(): "dark" | "light" {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}
