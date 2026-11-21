import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AppWindow,
  ChevronDown,
  ChevronRight,
  Code2,
  Download,
  FileText,
  FolderSearch,
  Gamepad2,
  HardDrive,
  Image,
  LayoutGrid,
  Package,
  RefreshCw,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import {
  errorText,
  storageApi,
  type EntityDetails,
  type EntitySummary,
  type StorageCategory,
  type StorageView,
  type Summary,
} from "../api";
import { bytes, date, middleTruncate, percent, plural } from "../format";
import { binName, useDelete } from "./DeleteFlow";
import { Dialog } from "./Dialog";
import { useToasts } from "./Toasts";

/** Category order, labels and brand colours (brand/tokens.json "storageCategories"). */
export const CATEGORIES: Record<
  StorageCategory,
  { label: string; viz: string; icon: LucideIcon; noun: [string, string]; blurb: string }
> = {
  apps: { label: "Apps", viz: "lilac", icon: AppWindow, noun: ["app", "apps"], blurb: "Programs with their data and caches" },
  games: { label: "Games", viz: "pistachio", icon: Gamepad2, noun: ["game", "games"], blurb: "Steam, Epic, Xbox and other games" },
  projects: { label: "Projects", viz: "mist", icon: Code2, noun: ["project", "projects"], blurb: "Code projects and their dependencies" },
  media: { label: "Media", viz: "powder", icon: Image, noun: ["group", "groups"], blurb: "Photos, videos and music" },
  documents: { label: "Documents", viz: "sand", icon: FileText, noun: ["folder", "folders"], blurb: "Documents, Desktop and OneDrive" },
  downloads: { label: "Downloads", viz: "peach", icon: Download, noun: ["folder", "folders"], blurb: "Your Downloads folder" },
  system: { label: "System", viz: "slate", icon: HardDrive, noun: ["item", "items"], blurb: "Windows, page and hibernation files, restore points" },
  caches: { label: "Caches & temp", viz: "blush", icon: Package, noun: ["item", "items"], blurb: "Temporary files and caches that rebuild themselves" },
  other: { label: "Other", viz: "stone", icon: LayoutGrid, noun: ["folder", "folders"], blurb: "Everything else, by folder" },
};

const osName = navigator.userAgent.includes("Mac") ? "macOS" : "Windows";

const ORDER: StorageCategory[] = ["apps", "games", "projects", "media", "documents", "downloads", "system", "caches", "other"];

const BUCKETS = ["app", "data", "cache", "build & deps"];

function breakdown(e: EntitySummary): string | null {
  const parts = e.buckets
    .map((b, i) => (b > 0 ? `${i === 0 && e.kind === "project" ? "source" : BUCKETS[i]} ${bytes(b)}` : null))
    .filter(Boolean);
  return parts.length > 1 ? parts.join(" · ") : null;
}

function Avatar({ name, viz }: { name: string; viz: string }) {
  const letter = (name.match(/[A-Za-z0-9]/)?.[0] ?? "?").toUpperCase();
  return (
    <div className="avatar" style={{ background: `var(--viz-${viz})` }} aria-hidden>
      {letter}
    </div>
  );
}

export function Storage({
  summary,
  onSummary,
  openInMap,
}: {
  summary: Summary;
  onSummary: (s: Summary) => void;
  openInMap: (node: number) => void;
}) {
  const [view, setView] = useState<StorageView | null>(null);
  const [open, setOpen] = useState<Set<StorageCategory>>(new Set(["apps"]));
  const [showAll, setShowAll] = useState<Set<StorageCategory>>(new Set());
  const [selected, setSelected] = useState<number | null>(null);
  const toasts = useToasts();

  const load = useCallback(() => {
    storageApi.storage().then(setView, (e) => toasts.err(errorText(e)));
  }, [toasts]);
  useEffect(load, [load, summary.generation]);

  const byCat = useMemo(() => {
    const m = new Map<StorageCategory, EntitySummary[]>();
    for (const e of view?.entities ?? []) {
      const list = m.get(e.category) ?? [];
      list.push(e);
      m.set(e.category, list);
    }
    return m;
  }, [view]);

  if (!view) {
    return (
      <div className="page">
        <div className="empty">
          <span className="spinner" />
          <div>Sorting everything into apps, games, projects and more…</div>
        </div>
      </div>
    );
  }

  const capacity = view.capacity ?? view.total;
  const hidden = (view.entities ?? []).filter((e) => e.kind === "hidden").reduce((a, e) => a + e.size, 0);
  const cats = ORDER.map((c) => view.categories.find((x) => x.category === c)!).filter((c) => c && c.size > 0);

  return (
    <div className="page storage">
      <section className="card storage-head">
        <div className="storage-title">
          <div>
            <div className="kicker">{view.isVolume ? "Storage" : "This folder"}</div>
            <div className="storage-amount">
              {bytes(view.isVolume ? view.used : view.total)}
              {view.isVolume && <span className="muted"> used of {bytes(capacity)}</span>}
            </div>
          </div>
          {view.isVolume && view.free != null && (
            <div className="storage-free">
              <b>{bytes(view.free)}</b>
              <span className="muted">free</span>
            </div>
          )}
        </div>
        <div className="stack-bar" role="img" aria-label="Space by category">
          {cats.map((c) => {
            const size = c.category === "system" ? c.size - hidden : c.size;
            return size > 0 ? (
              <span
                key={c.category}
                title={`${CATEGORIES[c.category].label}: ${bytes(size)}`}
                style={{ flexGrow: size, background: `var(--viz-${CATEGORIES[c.category].viz})` }}
              />
            ) : null;
          })}
          {hidden > 0 && <span className="hatched" title={`Hidden & reserved: ${bytes(hidden)}`} style={{ flexGrow: hidden }} />}
          {view.isVolume && (view.free ?? 0) > 0 && <span className="free" title={`Free: ${bytes(view.free)}`} style={{ flexGrow: view.free ?? 0 }} />}
        </div>
        <div className="stack-legend">
          {cats.map((c) => (
            <span key={c.category}>
              <i style={{ background: `var(--viz-${CATEGORIES[c.category].viz})` }} />
              {CATEGORIES[c.category].label} <b>{bytes(c.size)}</b>
            </span>
          ))}
          {view.isVolume && (
            <span>
              <i className="free" /> Free <b>{bytes(view.free)}</b>
            </span>
          )}
        </div>
      </section>

      <section className="card storage-list" aria-label="Storage by category">
        {cats.map((c) => {
          const meta = CATEGORIES[c.category];
          const list = byCat.get(c.category) ?? [];
          const isOpen = open.has(c.category);
          const all = showAll.has(c.category);
          const shown = all ? list : list.slice(0, 40);
          const Icon = meta.icon;
          return (
            <div key={c.category} className="cat">
              <button
                className="cat-row"
                aria-expanded={isOpen}
                onClick={() =>
                  setOpen((o) => {
                    const n = new Set(o);
                    if (n.has(c.category)) n.delete(c.category);
                    else n.add(c.category);
                    return n;
                  })
                }
              >
                <div className="cat-icon" style={{ background: `var(--viz-${meta.viz})` }}>
                  <Icon />
                </div>
                <div className="cat-main">
                  <div className="cat-name">{meta.label}</div>
                  <div className="cat-sub">
                    {plural(list.length, meta.noun[0], meta.noun[1])} · {meta.blurb}
                  </div>
                </div>
                <div className="cat-size">
                  <b>{bytes(c.size)}</b>
                  <span>{percent(c.size, view.total)}</span>
                </div>
                {isOpen ? <ChevronDown size={18} className="faint" /> : <ChevronRight size={18} className="faint" />}
              </button>
              {isOpen && (
                <div className="ent-list">
                  {shown.map((e) => (
                    <button key={e.id} className="ent-row" onClick={() => setSelected(e.id)}>
                      {e.kind === "hidden" ? (
                        <div className="avatar hatched" aria-hidden />
                      ) : (
                        <Avatar name={e.name} viz={meta.viz} />
                      )}
                      <div className="ent-main">
                        <div className="ent-name">{e.name}</div>
                        <div className="ent-sub">
                          {breakdown(e) ?? e.detail ?? (e.files ? plural(e.files, "file") : "")}
                          {e.kind === "project" && e.reclaimable > 0 && (
                            <span className="chip warn" style={{ marginLeft: 8 }}>
                              {bytes(e.reclaimable)} reclaimable
                            </span>
                          )}
                        </div>
                      </div>
                      <div className="ent-bar" aria-hidden>
                        <span style={{ width: percent(e.size, list[0]?.size || 1), background: `var(--viz-${meta.viz}-deep)` }} />
                      </div>
                      <b className="ent-size">{bytes(e.size)}</b>
                    </button>
                  ))}
                  {list.length > shown.length && (
                    <button className="link ent-more" onClick={() => setShowAll((s) => new Set(s).add(c.category))}>
                      Show {plural(list.length - shown.length, "more item")}
                    </button>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </section>
      <p className="faint" style={{ fontSize: 12.5, margin: "4px 4px 0" }}>
        {view.isVolume && view.used != null && view.total !== view.used
          ? `Every byte is counted once. Roomy counted ${bytes(Math.abs(view.total - view.used))} ${
              view.total > view.used ? "more" : "less"
            } than ${osName} reports as used (${bytes(view.used)}). That happens when files are compressed, deduplicated or shared between volumes, or change while Roomy looks.`
          : `Every byte is counted once. Categories add up to ${bytes(view.total)}${
              view.isVolume && view.used != null ? `, the space ${osName} reports as used` : ""
            }.`}
      </p>

      {selected !== null && (
        <EntityDialog
          id={selected}
          onClose={() => setSelected(null)}
          onChanged={(s) => {
            if (s) onSummary(s);
            load();
          }}
          openInMap={(n) => {
            setSelected(null);
            openInMap(n);
          }}
        />
      )}
    </div>
  );
}

function EntityDialog({
  id,
  onClose,
  onChanged,
  openInMap,
}: {
  id: number;
  onClose: () => void;
  onChanged: (s: Summary | null) => void;
  openInMap: (node: number) => void;
}) {
  const [d, setD] = useState<EntityDetails | null>(null);
  const [confirm, setConfirm] = useState<null | "uninstall" | "clearCache" | "cleanBuild">(null);
  const [busy, setBusy] = useState(false);
  const toasts = useToasts();
  const del = useDelete();

  useEffect(() => {
    storageApi.entity(id).then(setD, (e) => {
      toasts.err(errorText(e));
      onClose();
    });
  }, [id, onClose, toasts]);

  const run = async (action: "uninstall" | "clearCache" | "cleanBuild" | "refresh") => {
    setBusy(true);
    try {
      const r = await storageApi.action(id, action);
      toasts.ok(r.message);
      if (action !== "uninstall") {
        onChanged(r.summary);
        onClose();
      }
    } catch (e) {
      toasts.err(errorText(e));
    } finally {
      setBusy(false);
      setConfirm(null);
    }
  };

  if (!d) return null;
  const e = d.entity;
  const meta = CATEGORIES[e.category];
  const parts = e.buckets
    .map((b, i) => ({ b, label: i === 0 && e.kind === "project" ? "Source & files" : ["App files", "Data", "Cache", "Build & deps"][i], i }))
    .filter((p) => p.b > 0);
  const bucketLabel = (b: number) => (e.kind === "project" ? ["Project", "Data", "Cache", "Build & deps"] : ["App", "Data", "Cache", "Build"])[b];
  const uninstallWhere =
    d.uninstall?.type === "command"
      ? `${e.name}'s own uninstaller`
      : d.uninstall?.type === "url"
        ? d.uninstall.label
        : d.uninstall?.type === "launcher"
          ? d.uninstall.name
          : d.uninstall?.type === "settings"
            ? "Settings ▸ Apps"
            : d.uninstall?.type === "reveal"
              ? navigator.userAgent.includes("Mac")
                ? "the Finder, so you can remove it yourself"
                : "the folder, so you can remove it yourself"
              : "";

  if (confirm) {
    const isUninstall = confirm === "uninstall";
    const paths = confirm === "clearCache" ? d.cachePaths : d.buildPaths;
    return (
      <Dialog
        title={
          isUninstall
            ? `Uninstall ${e.name}?`
            : confirm === "clearCache"
              ? `Clear ${bytes(e.clearable)} of cache?`
              : `Remove ${bytes(e.cleanable)} of build output and dependencies?`
        }
        description={
          isUninstall
            ? `Roomy will open ${uninstallWhere}. Roomy never deletes an installed app's files itself.`
            : confirm === "clearCache"
              ? `${e.name} rebuilds its cache when it needs it. Close ${e.name} first; files it's using are skipped. This is permanent: moving a cache to the ${binName} wouldn't free space.`
              : "Your source code isn't touched. Reinstall or rebuild (for example npm install or cargo build) before running the project again. This is permanent."
        }
        onClose={() => !busy && setConfirm(null)}
        footer={
          <>
            <button className="btn" onClick={() => setConfirm(null)} disabled={busy} data-autofocus>
              Cancel
            </button>
            <button className={`btn ${isUninstall ? "primary" : "danger"}`} onClick={() => run(confirm)} disabled={busy}>
              {busy && <span className="spinner" />}
              {isUninstall
                ? d.uninstall?.type === "reveal"
                  ? "Show app"
                  : "Open uninstaller"
                : confirm === "clearCache"
                  ? "Clear cache"
                  : "Remove build & deps"}
            </button>
          </>
        }
      >
        {!isUninstall && (
          <div className="review-list">
            {paths.map(([p, size]) => (
              <div className="review-item" key={p}>
                <div className="ri-main">
                  <div className="ri-path" style={{ color: "var(--text)" }}>
                    {p}
                  </div>
                </div>
                {size > 0 && <b>{bytes(size)}</b>}
              </div>
            ))}
          </div>
        )}
      </Dialog>
    );
  }

  return (
    <Dialog
      wide
      title={e.name}
      description={[meta.label, e.detail, e.kind === "project" && e.mtime ? `changed ${date(e.mtime).toLowerCase()}` : null]
        .filter(Boolean)
        .join(" · ")}
      onClose={onClose}
      footer={
        <>
          <button className="btn ghost sm" onClick={() => run("refresh")} disabled={busy} title="Re-check these folders">
            <RefreshCw /> Refresh
          </button>
          <span style={{ flex: 1 }} />
          {e.canCleanBuild && (
            <button className="btn danger-ghost" onClick={() => setConfirm("cleanBuild")} disabled={busy}>
              <Trash2 /> Clean build & deps
            </button>
          )}
          {e.canClearCache && (
            <button className="btn" onClick={() => setConfirm("clearCache")} disabled={busy}>
              Clear cache
            </button>
          )}
          {e.canUninstall && (
            <button className="btn" onClick={() => setConfirm("uninstall")} disabled={busy}>
              Uninstall…
            </button>
          )}
        </>
      }
    >
      <div className="ent-total">{bytes(e.size)}</div>
      {parts.length > 1 && (
        <>
          <div className="stack-bar" style={{ height: 10, margin: "10px 0 8px" }}>
            {parts.map((p) => (
              <span
                key={p.i}
                style={{ flexGrow: p.b, background: p.i === 0 ? `var(--viz-${meta.viz})` : p.i === 1 ? "var(--viz-stone)" : p.i === 2 ? "var(--viz-blush)" : "var(--viz-peach)" }}
              />
            ))}
          </div>
          <div className="stack-legend" style={{ marginBottom: 14 }}>
            {parts.map((p) => (
              <span key={p.i}>
                <i style={{ background: p.i === 0 ? `var(--viz-${meta.viz})` : p.i === 1 ? "var(--viz-stone)" : p.i === 2 ? "var(--viz-blush)" : "var(--viz-peach)" }} />
                {p.label} <b>{bytes(p.b)}</b>
              </span>
            ))}
          </div>
        </>
      )}
      <h3 className="loc-title">{plural(d.locations.length, "location")}</h3>
      <div className="items">
        {d.locations.slice(0, 60).map((l) => (
          <div className="item" key={l.node}>
            <div className="grow">
              <div className="ellip" title={l.path} style={{ userSelect: "text" }}>
                {middleTruncate(l.path, 96)}
              </div>
              <div className="ipath">{parts.length > 1 ? bucketLabel(l.bucket) : l.isDir ? "Folder" : "File"}</div>
            </div>
            <b style={{ whiteSpace: "nowrap" }}>{bytes(l.bytes)}</b>
            {l.path.includes("\\") || l.path.startsWith("/") ? (
              <>
                <button className="btn ghost icon-btn sm" title="Show in folder" aria-label="Show in folder" onClick={() => del.reveal(l.path)}>
                  <FolderSearch />
                </button>
                <button className="btn ghost sm" onClick={() => openInMap(l.node)} title="Open this location in the map">
                  Map
                </button>
              </>
            ) : null}
          </div>
        ))}
      </div>
    </Dialog>
  );
}
