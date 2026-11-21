import { useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  ExternalLink,
  FolderSearch,
  Info,
  ListChecks,
  Lock,
  RefreshCw,
  ShieldCheck,
  TriangleAlert,
} from "lucide-react";
import {
  api,
  errorText,
  onDupesProgress,
  type CleanReport,
  type DupeGroup,
  type Suggestion,
  type Summary,
} from "../api";
import { bytes, date, middleTruncate, plural } from "../format";
import { Dialog } from "./Dialog";
import { binName, useDelete } from "./DeleteFlow";
import { useToasts } from "./Toasts";

type Sel = Record<string, Set<string>>;

const SECTIONS: { key: Suggestion["category"]; title: string; blurb: string }[] = [
  { key: "recommended", title: "Safe to clean", blurb: "Temporary files and caches your apps rebuild on their own." },
  { key: "developer", title: "Developer caches", blurb: "Package downloads your tools fetch again when needed." },
  { key: "review", title: "Review first", blurb: "Your own files. Nothing here is selected for you." },
];

function selectable(s: Suggestion) {
  return !s.needsAdmin && s.action !== "guide";
}

function initialSel(list: Suggestion[]): Sel {
  const sel: Sel = {};
  for (const s of list) {
    sel[s.id] = new Set(selectable(s) ? s.items.filter((i) => i.defaultSelected).map((i) => i.path) : []);
  }
  return sel;
}

function actionText(s: Suggestion): string {
  switch (s.action) {
    case "contents":
      return s.minAgeHours
        ? `Contents deleted permanently (files from the last ${s.minAgeHours >= 48 ? `${s.minAgeHours / 24} days` : "day"} kept)`
        : "Contents deleted permanently";
    case "whole":
      return "Deleted permanently";
    case "recycleBin":
      return `${binName} emptied`;
    default:
      return "";
  }
}

export function Suggestions({
  summary,
  suggestions,
  loading,
  reload,
  onSummary,
  isAdmin,
}: {
  summary: Summary;
  suggestions: Suggestion[] | null;
  loading: boolean;
  reload: () => void;
  onSummary: (s: Summary) => void;
  isAdmin: boolean;
}) {
  const toasts = useToasts();
  const del = useDelete();
  const [sel, setSel] = useState<Sel>({});
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [reviewing, setReviewing] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<{ report: CleanReport; trashed: number } | null>(null);
  const [dupes, setDupes] = useState<DupeGroup[] | null>(null);
  const [dupesRunning, setDupesRunning] = useState(false);
  const [dupeProgress, setDupeProgress] = useState({ done: 0, total: 0 });
  const [dupeSel, setDupeSel] = useState<Set<number>>(new Set());

  useEffect(() => {
    if (suggestions) setSel(initialSel(suggestions));
  }, [suggestions]);

  useEffect(() => {
    const un = onDupesProgress(setDupeProgress);
    return () => {
      void un.then((f) => f());
    };
  }, []);

  const list = useMemo(() => suggestions ?? [], [suggestions]);
  const selectedSize = (s: Suggestion) =>
    s.items.filter((i) => sel[s.id]?.has(i.path)).reduce((a, i) => a + i.size, 0);
  const dupeFiles = (dupes ?? []).flatMap((g) => g.files.map((f) => ({ ...f, size: g.size })));
  const dupeSelSize = dupeFiles.filter((f) => dupeSel.has(f.id)).reduce((a, f) => a + f.size, 0);
  const total = list.reduce((a, s) => a + selectedSize(s), 0) + dupeSelSize;
  const selCount = list.reduce((a, s) => a + (sel[s.id]?.size ?? 0), 0) + dupeSel.size;
  const recommendedTotal = list
    .filter((s) => s.category !== "review" && selectable(s))
    .reduce((a, s) => a + s.size, 0);

  const toggleAll = (s: Suggestion, on: boolean) =>
    setSel((p) => ({ ...p, [s.id]: new Set(on ? s.items.map((i) => i.path) : []) }));
  const toggleItem = (s: Suggestion, path: string) =>
    setSel((p) => {
      const n = new Set(p[s.id]);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return { ...p, [s.id]: n };
    });

  const findDupes = async () => {
    setDupesRunning(true);
    setDupeProgress({ done: 0, total: 0 });
    try {
      const g = await api.findDuplicates();
      setDupes(g);
      setDupeSel(new Set());
      setOpen((o) => new Set(o).add("dupes"));
    } catch (e) {
      toasts.err(errorText(e));
    } finally {
      setDupesRunning(false);
    }
  };

  const clean = async () => {
    setRunning(true);
    try {
      const selections = list.flatMap((s) =>
        [...(sel[s.id] ?? [])].map((path) => ({ suggestionId: s.id, path })),
      );
      const report = await api.cleanSuggestions(selections);
      let trashed = 0;
      if (dupeSel.size) {
        const r = await api.deleteNodes([...dupeSel], "trash");
        trashed = r.bytes;
        if (r.summary) report.summary = r.summary;
        for (const x of r.results) if (!x.ok && x.error) report.errors.push(x.error);
        setDupes((d) =>
          d
            ?.map((g) => ({ ...g, files: g.files.filter((f) => !dupeSel.has(f.id) || !r.results.find((x) => x.id === f.id)?.ok) }))
            .filter((g) => g.files.length > 1) ?? null,
        );
        setDupeSel(new Set());
      }
      if (report.summary) onSummary(report.summary);
      setResult({ report, trashed });
      reload();
    } catch (e) {
      toasts.err(errorText(e));
    } finally {
      setRunning(false);
      setReviewing(false);
    }
  };

  return (
    <div className="page">
      <div className="card sugg-hero">
        <div style={{ flex: 1 }}>
          {loading && !suggestions ? (
            <>
              <div className="amount">Looking for safe space…</div>
              <div className="muted">Checking temp folders, caches and the {binName}.</div>
            </>
          ) : (
            <>
              <div className="amount">
                <span className="room-mark">{bytes(total)}</span> selected
              </div>
              <div className="muted">
                {recommendedTotal > 0
                  ? `${bytes(recommendedTotal)} of caches and temporary files can go without affecting your apps or files.`
                  : "Nothing obvious to clean right now."}
              </div>
            </>
          )}
        </div>
        <button className="btn ghost" onClick={reload} disabled={loading} title="Check again">
          {loading ? <span className="spinner" /> : <RefreshCw />}
        </button>
        <button className="btn primary lg" disabled={!selCount || running} onClick={() => setReviewing(true)}>
          Review & clean
        </button>
      </div>

      {SECTIONS.map((sec) => {
        const items = list.filter((s) => s.category === sec.key);
        const showDupes = sec.key === "review";
        if (!items.length && !showDupes) return null;
        return (
          <section key={sec.key}>
            <div className="section-title">
              <h2>{sec.title}</h2>
              <span className="muted">{sec.blurb}</span>
            </div>
            <div className="sugg-list">
              {items.map((s) => {
                const chosen = sel[s.id]?.size ?? 0;
                const isOpen = open.has(s.id);
                const can = selectable(s);
                return (
                  <div className="card sugg" key={s.id}>
                    <div
                      className="sugg-head"
                      onClick={() => setOpen((o) => {
                        const n = new Set(o);
                        if (n.has(s.id)) n.delete(s.id);
                        else n.add(s.id);
                        return n;
                      })}
                    >
                      {s.action === "guide" ? (
                        <ExternalLink size={18} className="faint" />
                      ) : (
                        <input
                          type="checkbox"
                          className="check"
                          aria-label={`Select ${s.title}`}
                          disabled={!can}
                          checked={chosen > 0 && chosen === s.items.length}
                          ref={(el) => {
                            if (el) el.indeterminate = chosen > 0 && chosen < s.items.length;
                          }}
                          onClick={(e) => e.stopPropagation()}
                          onChange={(e) => toggleAll(s, e.target.checked)}
                        />
                      )}
                      <div style={{ minWidth: 0 }}>
                        <div className="sugg-title">
                          {s.title}
                          {s.needsAdmin && (
                            <span className="chip warn">
                              <Lock /> Needs administrator
                            </span>
                          )}
                          {s.category === "review" && <span className="chip">Opt-in</span>}
                        </div>
                        <div className="sugg-desc">{s.what}</div>
                      </div>
                      <div className="sugg-size">{bytes(s.size)}</div>
                      {isOpen ? <ChevronDown size={18} className="faint" /> : <ChevronRight size={18} className="faint" />}
                    </div>
                    {isOpen && (
                      <div className="sugg-body">
                        <div className="facts">
                          <div className="fact">
                            <div className="fk">
                              <ShieldCheck /> Why it's safe
                            </div>
                            {s.whySafe}
                          </div>
                          <div className="fact">
                            <div className="fk">
                              <Info /> What happens
                            </div>
                            {s.effect}
                          </div>
                          <div className="fact">
                            <div className="fk">
                              <ListChecks /> How
                            </div>
                            {s.action === "guide" ? "Done by Windows' own cleanup tool." : actionText(s) + "."}
                          </div>
                        </div>
                        {s.note && (
                          <div className="info-box" style={{ margin: 0 }}>
                            <Info />
                            <div>{s.note}</div>
                          </div>
                        )}
                        {s.needsAdmin && (
                          <div className="info-box" style={{ margin: 0 }}>
                            <Lock />
                            <div>
                              Windows only lets administrators clean this.{" "}
                              {!isAdmin && (
                                <button className="link" onClick={() => api.relaunchElevated().catch((e) => toasts.err(errorText(e)))}>
                                  Restart Roomy as administrator
                                </button>
                              )}
                            </div>
                          </div>
                        )}
                        {s.action === "guide" ? (
                          <div>
                            <button className="btn" onClick={() => api.openStorageSettings().catch((e) => toasts.err(errorText(e)))}>
                              <ExternalLink /> Open Storage settings
                            </button>
                          </div>
                        ) : (
                          <div className="items">
                            {s.items.map((it) => (
                              <label className="item" key={it.path}>
                                <input
                                  type="checkbox"
                                  className="check"
                                  disabled={!can}
                                  checked={sel[s.id]?.has(it.path) ?? false}
                                  onChange={() => toggleItem(s, it.path)}
                                />
                                <div className="grow">
                                  <div className="ellip">{it.label}</div>
                                  <div className="ipath ellip" title={it.path}>
                                    {it.path === "recycle-bin" ? `Everything in the ${binName}` : middleTruncate(it.path, 100)}
                                    {s.id === "project-junk" && ` · changed ${date(it.mtime).toLowerCase()}`}
                                  </div>
                                </div>
                                {it.path !== "recycle-bin" && (
                                  <button
                                    className="btn ghost icon-btn sm"
                                    title="Show in folder"
                                    aria-label="Show in folder"
                                    onClick={(e) => {
                                      e.preventDefault();
                                      del.reveal(it.path);
                                    }}
                                  >
                                    <FolderSearch />
                                  </button>
                                )}
                                <b>{bytes(it.size)}</b>
                              </label>
                            ))}
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
              {showDupes && (
                <DupesCard
                  dupes={dupes}
                  running={dupesRunning}
                  progress={dupeProgress}
                  selected={dupeSel}
                  setSelected={setDupeSel}
                  onFind={findDupes}
                  open={open.has("dupes")}
                  toggleOpen={() =>
                    setOpen((o) => {
                      const n = new Set(o);
                      if (n.has("dupes")) n.delete("dupes");
                      else n.add("dupes");
                      return n;
                    })
                  }
                  reveal={del.reveal}
                  summary={summary}
                />
              )}
            </div>
          </section>
        );
      })}

      {reviewing && (
        <Dialog
          wide
          title={`Clean up ${bytes(total)}?`}
          description="Here's exactly what Roomy will remove. Nothing happens until you confirm."
          onClose={() => !running && setReviewing(false)}
          footer={
            <>
              <span className="left">{plural(selCount, "item")}</span>
              <button className="btn" onClick={() => setReviewing(false)} disabled={running} data-autofocus>
                Cancel
              </button>
              <button className="btn primary" onClick={clean} disabled={running}>
                {running && <span className="spinner" />}
                {running ? "Cleaning…" : `Clean up ${bytes(total)}`}
              </button>
            </>
          }
        >
          {list.some((s) => (sel[s.id]?.size ?? 0) > 0 && s.action !== "recycleBin") && (
            <div className="info-box">
              <Info />
              <div>
                Caches and temporary files are deleted permanently: moving them to the {binName} wouldn't free any space.
                Files that are in use are skipped.
              </div>
            </div>
          )}
          {list
            .filter((s) => (sel[s.id]?.size ?? 0) > 0)
            .map((s) => (
              <div className="review-group" key={s.id}>
                <h3>
                  {s.title} · {bytes(selectedSize(s))}
                  <span className="chip">{actionText(s)}</span>
                </h3>
                <div className="review-list">
                  {s.items
                    .filter((i) => sel[s.id].has(i.path))
                    .map((i) => (
                      <div className="review-item" key={i.path}>
                        <div className="ri-main">
                          <div className="ri-name">{i.label}</div>
                          <div className="ri-path">{i.path === "recycle-bin" ? `Everything in the ${binName}` : i.path}</div>
                        </div>
                        <b>{bytes(i.size)}</b>
                      </div>
                    ))}
                </div>
              </div>
            ))}
          {dupeSel.size > 0 && (
            <div className="review-group">
              <h3>
                Duplicate copies · {bytes(dupeSelSize)}
                <span className="chip">Moved to {binName}</span>
              </h3>
              <div className="review-list">
                {dupeFiles
                  .filter((f) => dupeSel.has(f.id))
                  .map((f) => (
                    <div className="review-item" key={f.id}>
                      <div className="ri-main">
                        <div className="ri-path" style={{ color: "var(--text)" }}>
                          {f.path}
                        </div>
                      </div>
                      <b>{bytes(f.size)}</b>
                    </div>
                  ))}
              </div>
            </div>
          )}
        </Dialog>
      )}

      {result && (
        <Dialog
          title={result.report.freed + result.trashed > 0 ? "Room made" : "Nothing was removed"}
          onClose={() => setResult(null)}
          footer={
            <button className="btn primary" onClick={() => setResult(null)} data-autofocus>
              Done
            </button>
          }
        >
          <div className="result-hero">
            <div className="amount room-mark">{bytes(result.report.freed)}</div>
            <div className="muted">freed on your drive</div>
            {result.trashed > 0 && (
              <div className="muted" style={{ marginTop: 6 }}>
                plus {bytes(result.trashed)} moved to the {binName}
              </div>
            )}
          </div>
          {(result.report.failed > 0 || result.report.errors.length > 0) && (
            <div className="warn-box" style={{ background: "var(--amber-soft)" }}>
              <TriangleAlert style={{ color: "var(--amber)" }} />
              <div>
                {result.report.failed > 0 && <div>{plural(result.report.failed, "item was", "items were")} in use or protected and left alone. That's normal for caches of open apps.</div>}
                {result.report.errors.slice(0, 4).map((e, i) => (
                  <div key={i} className="muted" style={{ marginTop: 4 }}>
                    {e}
                  </div>
                ))}
              </div>
            </div>
          )}
        </Dialog>
      )}
    </div>
  );
}

function DupesCard({
  dupes,
  running,
  progress,
  selected,
  setSelected,
  onFind,
  open,
  toggleOpen,
  reveal,
  summary,
}: {
  dupes: DupeGroup[] | null;
  running: boolean;
  progress: { done: number; total: number };
  selected: Set<number>;
  setSelected: (s: Set<number>) => void;
  onFind: () => void;
  open: boolean;
  toggleOpen: () => void;
  reveal: (p: string) => void;
  summary: Summary;
}) {
  const wasted = (dupes ?? []).reduce((a, g) => a + g.wasted, 0);
  const toggle = (id: number) => {
    const n = new Set(selected);
    if (n.has(id)) n.delete(id);
    else n.add(id);
    setSelected(n);
  };
  return (
    <div className="card sugg">
      <div className="sugg-head" onClick={dupes?.length ? toggleOpen : undefined}>
        <Copy size={18} className="faint" />
        <div style={{ minWidth: 0 }}>
          <div className="sugg-title">
            Duplicate files <span className="chip">Opt-in</span>
          </div>
          <div className="sugg-desc">
            {dupes === null
              ? `Find identical copies of files over 10 MB in ${summary.rootPath}. Roomy compares their full contents.`
              : dupes.length
                ? `${plural(dupes.length, "set")} of identical files. Pick the copies you don't need; they go to the ${binName}.`
                : "No duplicate large files found."}
          </div>
        </div>
        {dupes === null || running ? (
          <button
            className="btn"
            style={{ marginLeft: "auto" }}
            disabled={running}
            onClick={(e) => {
              e.stopPropagation();
              onFind();
            }}
          >
            {running ? <span className="spinner" /> : <Copy />}
            {running
              ? progress.total
                ? `Comparing… ${Math.floor((progress.done / Math.max(progress.total, 1)) * 100)}%`
                : "Comparing…"
              : "Find duplicates"}
          </button>
        ) : (
          <>
            <div className="sugg-size">{bytes(wasted)}</div>
            {dupes.length > 0 &&
              (open ? <ChevronDown size={18} className="faint" /> : <ChevronRight size={18} className="faint" />)}
          </>
        )}
      </div>
      {open && dupes && dupes.length > 0 && (
        <div className="sugg-body">
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <button
              className="btn sm"
              onClick={() => setSelected(new Set(dupes.flatMap((g) => g.files.slice(1).map((f) => f.id))))}
            >
              Select all but the oldest copy
            </button>
            <button className="btn sm ghost" onClick={() => setSelected(new Set())}>
              Clear
            </button>
          </div>
          <div className="items">
            {dupes.map((g, gi) => (
              <div className="dupe-group" key={gi}>
                <div className="dupe-head">
                  {plural(g.files.length, "copy", "copies")} · {bytes(g.size)} each
                </div>
                {g.files.map((f, fi) => {
                  const keepingNone = g.files.every((x) => x.id === f.id || selected.has(x.id));
                  return (
                    <label className="item" key={f.id} style={{ borderBottom: "none" }}>
                      <input
                        type="checkbox"
                        className="check"
                        checked={selected.has(f.id)}
                        // Always keep at least one copy.
                        disabled={!selected.has(f.id) && keepingNone}
                        onChange={() => toggle(f.id)}
                      />
                      <div className="grow">
                        <div className="ellip" title={f.path}>
                          {middleTruncate(f.path, 100)}
                        </div>
                        <div className="ipath">
                          Modified {date(f.mtime).toLowerCase()}
                          {fi === 0 && " · oldest"}
                        </div>
                      </div>
                      <button
                        className="btn ghost icon-btn sm"
                        title="Show in folder"
                        aria-label="Show in folder"
                        onClick={(e) => {
                          e.preventDefault();
                          reveal(f.path);
                        }}
                      >
                        <FolderSearch />
                      </button>
                    </label>
                  );
                })}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
