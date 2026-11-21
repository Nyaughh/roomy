import { useState } from "react";
import { Monitor, Moon, Sun } from "lucide-react";
import { getThemePref, setThemePref, type ThemePref } from "../theme";

const ORDER: ThemePref[] = ["light", "dark", "system"];
const LABEL: Record<ThemePref, string> = {
  system: "Theme: follow system",
  dark: "Theme: dark",
  light: "Theme: light",
};

export function ThemeToggle() {
  const [pref, setPref] = useState<ThemePref>(getThemePref());
  const Icon = pref === "system" ? Monitor : pref === "dark" ? Moon : Sun;
  return (
    <button
      className="btn ghost icon-btn"
      title={LABEL[pref]}
      aria-label={`${LABEL[pref]}. Click to change.`}
      onClick={() => {
        const next = ORDER[(ORDER.indexOf(pref) + 1) % ORDER.length];
        setThemePref(next);
        setPref(next);
      }}
    >
      <Icon />
    </button>
  );
}
