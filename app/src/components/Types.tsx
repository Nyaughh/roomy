import { useEffect, useState } from "react";
import {
  AppWindow,
  Archive,
  Code2,
  Database,
  FileQuestion,
  FileText,
  Gamepad2,
  HardDrive,
  Image,
  Music,
  Server,
  Settings2,
  Video,
} from "lucide-react";
import { api, errorText, type Summary, type TypeStat } from "../api";
import { bytes, count, percent } from "../format";
import { useToasts } from "./Toasts";

export const CATEGORY: Record<string, { color: string; icon: typeof Video; blurb: string }> = {
  Video: { color: "#8B93FF", icon: Video, blurb: "Movies, recordings and clips" },
  Images: { color: "#6EE7C5", icon: Image, blurb: "Photos, screenshots and artwork" },
  Audio: { color: "#F49AC1", icon: Music, blurb: "Music, podcasts and recordings" },
  Documents: { color: "#7CC4FA", icon: FileText, blurb: "PDFs, office files and text" },
  Archives: { color: "#F5D97A", icon: Archive, blurb: "Zips, disk images and installers' packages" },
  "Apps & code binaries": { color: "#C4A1FF", icon: AppWindow, blurb: "Programs, libraries and compiled code" },
  Code: { color: "#5ED3D1", icon: Code2, blurb: "Source code, configs and scripts" },
  "Virtual disks": { color: "#FFB38A", icon: Server, blurb: "Virtual machine and WSL disks" },
  "Game data": { color: "#B5E07A", icon: Gamepad2, blurb: "Game assets and packages" },
  "Data & logs": { color: "#93C5FD", icon: Database, blurb: "Databases, logs, caches and app data" },
  System: { color: "#A5B4FC", icon: Settings2, blurb: "Page file, hibernation file and drivers" },
  "Hidden system space": { color: "#6B7190", icon: HardDrive, blurb: "Space in use that no folder shows" },
  Other: { color: "#9AA0B8", icon: FileQuestion, blurb: "Everything else" },
};

export function Types({ summary }: { summary: Summary }) {
  const [types, setTypes] = useState<TypeStat[] | null>(null);
  const toasts = useToasts();
  useEffect(() => {
    api.fileTypes().then(setTypes, (e) => toasts.err(errorText(e)));
  }, [summary.generation, toasts]);

  if (!types) return <div className="page" />;
  const total = types.reduce((s, t) => s + t.size, 0);

  return (
    <div className="page">
      <div className="section-title">
        <h2>What's taking up space</h2>
        <span className="muted">
          {bytes(total)} across {count(summary.files)} files, grouped by kind
        </span>
      </div>
      <div className="type-bar" role="img" aria-label="Share of space by file type">
        {types.map((t) => (
          <span
            key={t.category}
            title={`${t.category}: ${bytes(t.size)}`}
            style={{
              width: percent(t.size, total),
              background:
                t.category === "Hidden system space"
                  ? "repeating-linear-gradient(-45deg, var(--faint) 0 3px, transparent 3px 6px)"
                  : (CATEGORY[t.category]?.color ?? "#9AA0B8"),
            }}
          />
        ))}
      </div>
      <div className="type-grid">
        {types.map((t) => {
          const meta = CATEGORY[t.category] ?? CATEGORY.Other;
          const Icon = meta.icon;
          return (
            <div className="card type-card" key={t.category}>
              <div className="tc-head">
                <div
                  className="drive-icon"
                  style={{ width: 34, height: 34, background: `${meta.color}26`, color: meta.color }}
                >
                  <Icon size={17} />
                </div>
                <div>
                  <div className="tc-name">{t.category}</div>
                  <div className="faint" style={{ fontSize: 12 }}>
                    {meta.blurb}
                  </div>
                </div>
                <div className="tc-size">{bytes(t.size)}</div>
              </div>
              <div className="bar" style={{ height: 6 }}>
                <span style={{ width: percent(t.size, total), background: meta.color, borderRadius: 99 }} />
              </div>
              <div className="muted" style={{ fontSize: 12.5 }}>
                {percent(t.size, total)} of scanned space
                {t.count > 0 && ` · ${count(t.count)} files`}
              </div>
              <div className="exts">
                {t.exts.slice(0, 8).map((e) => (
                  <span className="chip" key={e.ext} title={`${count(e.count)} files`}>
                    {e.ext} <span style={{ color: "var(--text)" }}>{bytes(e.size)}</span>
                  </span>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
