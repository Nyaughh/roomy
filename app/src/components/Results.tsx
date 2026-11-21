import { useCallback, useEffect, useState } from "react";
import { FileStack, House, LayoutGrid, ListChecks, PieChart, RotateCw } from "lucide-react";
import { api, errorText, type AppInfo, type Suggestion, type Summary } from "../api";
import { bytes, count, duration, percent, plural } from "../format";
import { DeleteProvider } from "./DeleteFlow";
import { Dialog } from "./Dialog";
import { Explorer } from "./Explorer";
import { Largest } from "./Largest";
import { Mark } from "./Logo";
import { Suggestions } from "./Suggestions";
import { ThemeToggle } from "./ThemeToggle";
import { useToasts } from "./Toasts";
import { Types } from "./Types";

type Tab = "explore" | "types" | "largest" | "suggestions";

export function Results({
  info,
  summary,
  setSummary,
  onRescan,
  onHome,
}: {
  info: AppInfo | null;
  summary: Summary;
  setSummary: (s: Summary) => void;
  onRescan: () => void;
  onHome: () => void;
}) {
  const [tab, setTab] = useState<Tab>("explore");
  const [currentId, setCurrentId] = useState(0);
  const [suggestions, setSuggestions] = useState<Suggestion[] | null>(null);
  const [suggLoading, setSuggLoading] = useState(false);
  const [visited, setVisited] = useState<Set<Tab>>(new Set(["explore"]));
  const [showGap, setShowGap] = useState(false);
  const toasts = useToasts();

  const loadSuggestions = useCallback(() => {
    setSuggLoading(true);
    api
      .suggestions()
      .then(setSuggestions, (e) => toasts.err(errorText(e)))
      .finally(() => setSuggLoading(false));
  }, [toasts]);

  useEffect(loadSuggestions, [loadSuggestions]);

  const go = (t: Tab) => {
    setTab(t);
    setVisited((v) => new Set(v).add(t));
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
      const map: Record<string, Tab> = { "1": "explore", "2": "types", "3": "largest", "4": "suggestions" };
      if (map[e.key]) {
        e.preventDefault();
        go(map[e.key]);
      } else if (e.key.toLowerCase() === "r") {
        e.preventDefault();
        onRescan();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onRescan]);

  const reclaimable = (suggestions ?? [])
    .filter((s) => s.category !== "review" && !s.needsAdmin && s.action !== "guide")
    .reduce((a, s) => a + s.items.filter((i) => i.defaultSelected).reduce((x, i) => x + i.size, 0), 0);

  const used = summary.used;
  const hidden = Math.max(0, summary.gap);
  const total = summary.volumeTotal ?? 0;
  const title = summary.isVolume ? `Drive ${summary.rootPath.replace(/\\$/, "")}` : summary.rootPath.split(/[\\/]/).filter(Boolean).pop() ?? summary.rootPath;

  const nav: { key: Tab; label: string; icon: typeof LayoutGrid; kbd: string }[] = [
    { key: "explore", label: "Explore", icon: LayoutGrid, kbd: "1" },
    { key: "types", label: "File types", icon: PieChart, kbd: "2" },
    { key: "largest", label: "Largest files", icon: FileStack, kbd: "3" },
    { key: "suggestions", label: "Suggestions", icon: ListChecks, kbd: "4" },
  ];

  return (
    <DeleteProvider onChanged={setSummary}>
      <div className="shell">
        <aside className="sidebar">
          <div className="brand">
            <Mark size={20} /> Roomy
          </div>
          <nav aria-label="Views" style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            {nav.map((n) => (
              <button
                key={n.key}
                className={`nav-item ${tab === n.key ? "active" : ""}`}
                onClick={() => go(n.key)}
                aria-current={tab === n.key ? "page" : undefined}
                title={`${n.label} (Ctrl+${n.kbd})`}
              >
                <n.icon /> {n.label}
                {n.key === "suggestions" && reclaimable > 0 && <span className="badge">{bytes(reclaimable, 0)}</span>}
                {n.key === "suggestions" && suggLoading && !suggestions && (
                  <span className="spinner" style={{ marginLeft: "auto", width: 14, height: 14 }} />
                )}
              </button>
            ))}
          </nav>
          <div className="side-spacer" />
          <div className="space-card">
            {summary.isVolume && used != null ? (
              <>
                <div className="title">Where your space went</div>
                <div className="bar" style={{ height: 10 }}>
                  <span className="fill" style={{ width: percent(summary.scannedBytes, total), borderRadius: 0 }} />
                  <span className="hidden-part" style={{ width: percent(hidden, total) }} />
                </div>
                <div className="legend">
                  <div>
                    <span className="dot files" /> Files & folders <b>{bytes(summary.scannedBytes)}</b>
                  </div>
                  <div>
                    <span className="dot hidden" /> System & hidden <b>{bytes(hidden)}</b>
                  </div>
                  <div>
                    <span className="dot free" /> Free <b>{bytes(summary.volumeFree)}</b>
                  </div>
                </div>
                <button className="link" style={{ textAlign: "left", fontSize: 12.5 }} onClick={() => setShowGap(true)}>
                  What's in “System & hidden”?
                </button>
              </>
            ) : (
              <>
                <div className="title">This folder</div>
                <div className="legend">
                  <div>
                    Size on disk <b>{bytes(summary.scannedBytes)}</b>
                  </div>
                  <div>
                    Files <b>{count(summary.files)}</b>
                  </div>
                  {summary.deniedDirs > 0 && (
                    <div>
                      Couldn't open <b>{plural(summary.deniedDirs, "folder")}</b>
                    </div>
                  )}
                </div>
              </>
            )}
          </div>
          <div className="side-foot">
            <button className="btn ghost sm" onClick={onHome} title="Pick another drive or folder">
              <House /> New scan
            </button>
            <span style={{ flex: 1 }} />
            <ThemeToggle />
          </div>
        </aside>
        <main className="main">
          <header className="topbar">
            <div style={{ minWidth: 0 }}>
              <h1 className="ellip">{title}</h1>
              <div className="sub ellip">
                {summary.isVolume && used != null
                  ? `${bytes(used)} used of ${bytes(total)} · `
                  : `${bytes(summary.scannedBytes)} · `}
                {count(summary.files)} files in {count(summary.dirs)} folders · scanned in {duration(summary.elapsedMs)}
              </div>
            </div>
            <div className="spacer" />
            {info?.platform === "windows" && !info.elevated && summary.deniedDirs > 0 && (
              <button
                className="btn ghost sm"
                title="Restart with administrator rights to look inside protected folders"
                onClick={() => api.relaunchElevated().catch((e) => toasts.err(errorText(e)))}
              >
                Run as admin
              </button>
            )}
            <button className="btn sm" onClick={onRescan} title="Scan again (Ctrl+R)">
              <RotateCw /> Rescan
            </button>
          </header>
          <div className="content">
            {tab === "explore" && <Explorer summary={summary} currentId={currentId} setCurrentId={setCurrentId} />}
            {tab === "types" && <Types summary={summary} />}
            {tab === "largest" && <Largest summary={summary} />}
            {visited.has("suggestions") && (
              <div style={{ display: tab === "suggestions" ? "contents" : "none" }}>
                <Suggestions
                  summary={summary}
                  suggestions={suggestions}
                  loading={suggLoading}
                  reload={loadSuggestions}
                  onSummary={setSummary}
                  isAdmin={!!info?.elevated}
                />
              </div>
            )}
          </div>
        </main>
      </div>
      {showGap && (
        <Dialog
          title="Nothing hidden: here's the rest"
          description={`Windows says ${bytes(used)} of this drive is in use. Roomy found ${bytes(summary.scannedBytes)} in files and folders. The difference is shown on the map as “System & unreadable space”.`}
          onClose={() => setShowGap(false)}
          footer={
            <button className="btn primary" onClick={() => setShowGap(false)} data-autofocus>
              Got it
            </button>
          }
        >
          {summary.gap < 0 && (
            <div className="info-box">
              <div>
                Files add up to {bytes(-summary.gap)} more than the space in use. That happens when files are
                compressed or deduplicated by the system.
              </div>
            </div>
          )}
          <div className="gap-rows" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            {summary.gapParts.map((p) => (
              <div key={p.label}>
                <div className="gap-row">
                  <span style={{ fontWeight: 650 }}>{p.label}</span>
                  <b>{bytes(p.bytes)}</b>
                </div>
                <div className="muted" style={{ fontSize: 13 }}>
                  {p.explain}
                </div>
              </div>
            ))}
            {summary.hardlinkBytes > 0 && (
              <div>
                <div className="gap-row">
                  <span style={{ fontWeight: 650 }}>Hard links counted once</span>
                  <b>{bytes(summary.hardlinkBytes)}</b>
                </div>
                <div className="muted" style={{ fontSize: 13 }}>
                  Some files appear in more than one folder but are stored only once. Roomy counts them once, so
                  nothing is double-counted.
                </div>
              </div>
            )}
            {summary.linksSkipped > 0 && (
              <div className="muted" style={{ fontSize: 13 }}>
                {plural(summary.linksSkipped, "shortcut link")} (junctions and symlinks) point elsewhere. Roomy
                doesn't follow them, so nothing is counted twice.
              </div>
            )}
            {summary.deniedSamples.length > 0 && (
              <div>
                <div style={{ fontWeight: 650, marginBottom: 6 }}>
                  {plural(summary.deniedDirs, "folder")} Roomy wasn't allowed to open
                </div>
                <div className="mono faint" style={{ display: "flex", flexDirection: "column", gap: 3, userSelect: "text" }}>
                  {summary.deniedSamples.slice(0, 12).map((d) => (
                    <span key={d} className="ellip" title={d}>
                      {d}
                    </span>
                  ))}
                  {summary.deniedDirs > 12 && <span>…and {count(summary.deniedDirs - 12)} more</span>}
                </div>
              </div>
            )}
          </div>
        </Dialog>
      )}
    </DeleteProvider>
  );
}
