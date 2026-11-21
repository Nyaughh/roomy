import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { api, onScanProgress, type ScanProgress } from "../api";
import { bytes, count, duration, middleTruncate } from "../format";

export function Scanning({ path, onCancel }: { path: string; onCancel: () => void }) {
  const [p, setP] = useState<ScanProgress | null>(null);

  useEffect(() => {
    const un = onScanProgress(setP);
    return () => {
      void un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void api.cancelScan();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const frac = p?.target ? Math.min(0.99, p.bytes / p.target) : null;
  const R = 80;
  const C = 2 * Math.PI * R;

  return (
    <div className="scanning">
      <div className="scan-card" aria-live="polite">
        <div className="scan-ring">
          <svg viewBox="0 0 180 180" aria-hidden="true">
            <defs>
              <linearGradient id="ring" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0" stopColor="#8B93FF" />
                <stop offset="1" stopColor="#6EE7C5" />
              </linearGradient>
            </defs>
            <circle cx="90" cy="90" r={R} fill="none" stroke="var(--raised-2)" strokeWidth="10" />
            {frac !== null ? (
              <circle
                cx="90"
                cy="90"
                r={R}
                fill="none"
                stroke="url(#ring)"
                strokeWidth="10"
                strokeLinecap="round"
                strokeDasharray={`${C * frac} ${C}`}
                style={{ transition: "stroke-dasharray 0.3s ease" }}
              />
            ) : (
              <g className="spin">
                <circle
                  cx="90"
                  cy="90"
                  r={R}
                  fill="none"
                  stroke="url(#ring)"
                  strokeWidth="10"
                  strokeLinecap="round"
                  strokeDasharray={`${C * 0.25} ${C}`}
                />
              </g>
            )}
          </svg>
          <div className="ring-center">
            <div className="big">{frac !== null ? `${Math.floor(frac * 100)}%` : bytes(p?.bytes ?? 0)}</div>
            <div className="muted" style={{ fontSize: 12.5 }}>
              {frac !== null ? `${bytes(p?.bytes ?? 0)} of ${bytes(p?.target)}` : "found so far"}
            </div>
          </div>
        </div>
        <h2>Mapping {middleTruncate(path, 48)}</h2>
        <div className="muted">Counting every file. You can keep using your computer.</div>
        <div className="scan-stats">
          <div className="stat">
            <div className="v">{count(p?.files ?? 0)}</div>
            <div className="k">files</div>
          </div>
          <div className="stat">
            <div className="v">{count(p?.dirs ?? 0)}</div>
            <div className="k">folders</div>
          </div>
          <div className="stat">
            <div className="v">{duration(p?.elapsedMs ?? 0)}</div>
            <div className="k">elapsed</div>
          </div>
        </div>
        <div className="scan-path" title={p?.current}>
          {p?.current ? middleTruncate(p.current, 90) : " "}
        </div>
        <button
          className="btn"
          onClick={() => {
            void api.cancelScan();
            onCancel();
          }}
        >
          <X /> Cancel <kbd>Esc</kbd>
        </button>
      </div>
    </div>
  );
}
