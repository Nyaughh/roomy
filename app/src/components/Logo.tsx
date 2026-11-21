import { useId } from "react";

export function Mark({ size = 28 }: { size?: number }) {
  const id = useId().replace(/:/g, "");
  return (
    <svg width={size} height={size} viewBox="0 0 512 512" aria-hidden="true">
      <defs>
        <linearGradient id={`bg${id}`} x1="40" y1="24" x2="472" y2="488" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#8B93FF" />
          <stop offset="1" stopColor="#6EE7C5" />
        </linearGradient>
      </defs>
      <rect x="16" y="16" width="480" height="480" rx="124" fill={`url(#bg${id})`} />
      <rect x="104" y="104" width="148" height="304" rx="40" fill="#14162B" />
      <rect x="270" y="104" width="138" height="138" rx="40" fill="#14162B" />
      <rect
        x="279"
        y="269"
        width="120"
        height="130"
        rx="34"
        fill="none"
        stroke="#14162B"
        strokeOpacity="0.42"
        strokeWidth="18"
        strokeDasharray="26 22"
        strokeLinecap="round"
      />
      <path
        d="M339 300 C342 324 346 328 370 334 C346 340 342 344 339 368 C336 344 332 340 308 334 C332 328 336 324 339 300 Z"
        fill="#14162B"
      />
    </svg>
  );
}
