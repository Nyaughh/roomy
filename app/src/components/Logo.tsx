/** The Roomy glyph: three cells of a drive, one freed (butter). Ink cells use currentColor. */
export function Mark({ size = 24 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 288 288" aria-hidden="true" style={{ flex: "none" }}>
      <rect x="0" y="0" width="130" height="288" rx="22" fill="currentColor" />
      <rect x="158" y="0" width="130" height="130" rx="22" fill="currentColor" />
      <rect x="158" y="158" width="130" height="130" rx="22" fill="#F6D374" />
    </svg>
  );
}
