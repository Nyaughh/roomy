import { useRef } from "react";
import { markBroken, useIcon } from "../icons";

/**
 * The rounded tile for an app, game or folder: the real icon when Roomy can
 * find one, otherwise the first letter on the category colour.
 */
export function EntityIcon({
  name,
  viz,
  iconKey,
  size = "md",
}: {
  name: string;
  viz: string;
  iconKey: string | null;
  size?: "md" | "lg";
}) {
  const ref = useRef<HTMLDivElement>(null);
  const src = useIcon(iconKey, ref);
  if (src && iconKey) {
    // Launcher art (JPEG) is a full square picture; icons sit on a soft tile.
    const art = src.startsWith("data:image/jpeg");
    return (
      <div
        ref={ref}
        className={`avatar ${size} has-icon${art ? " art" : ""}`}
        style={{ background: `color-mix(in srgb, var(--viz-${viz}) 38%, var(--surface))` }}
        aria-hidden
      >
        <img src={src} alt="" draggable={false} decoding="async" onError={() => markBroken(iconKey)} />
      </div>
    );
  }
  const letter = (name.match(/[A-Za-z0-9]/)?.[0] ?? "?").toUpperCase();
  return (
    <div ref={ref} className={`avatar ${size}`} style={{ background: `var(--viz-${viz})` }} aria-hidden>
      {letter}
    </div>
  );
}
