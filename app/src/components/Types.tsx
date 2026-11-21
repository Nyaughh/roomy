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

/** Category → brand viz color name (brand/BRAND.md). */
export const CATEGORY: Record<string, { color: string; icon: typeof Video; blurb: string }> = {
  Video: { color: "powder", icon: Video, blurb: "Movies, recordings and clips" },
  Images: { color: "sage", icon: Image, blurb: "Photos, screenshots and artwork" },
  Audio: { color: "blush", icon: Music, blurb: "Music, podcasts and recordings" },
  Documents: { color: "sand", icon: FileText, blurb: "PDFs, office files and text" },
  Archives: { color: "peach", icon: Archive, blurb: "Zips, disk images and installers' packages" },
  "Apps & code binaries": { color: "lilac", icon: AppWindow, blurb: "Programs, libraries and compiled code" },
  Code: { color: "mist", icon: Code2, blurb: "Source code, configs and scripts" },
  "Virtual disks": { color: "cocoa", icon: Server, blurb: "Virtual machine and WSL disks" },
  "Game data": { color: "pistachio", icon: Gamepad2, blurb: "Game assets and packages" },
  "Data & logs": { color: "mauve", icon: Database, blurb: "Databases, logs, caches and app data" },
  System: { color: "slate", icon: Settings2, blurb: "Page file, hibernation file and drivers" },
  "Hidden system space": { color: "stone", icon: HardDrive, blurb: "Space in use that no folder shows" },
  Other: { color: "stone", icon: FileQuestion, blurb: "Everything else" },
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
                  ? "repeating-linear-gradient(-45deg, var(--viz-hatch) 0 3px, var(--viz-stone) 3px 6px)"
                  : `var(--viz-${CATEGORY[t.category]?.color ?? "stone"})`,
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
                  style={{ width: 34, height: 34, background: `var(--viz-${meta.color})`, color: "var(--viz-label)" }}
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
                <span style={{ width: percent(t.size, total), background: `var(--viz-${meta.color}-deep)`, borderRadius: 99 }} />
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
