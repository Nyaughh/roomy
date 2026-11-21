import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ChevronRight,
  CircleHelp,
  File,
  Folder,
  FolderLock,
  FolderSearch,
  HardDrive,
  Link2,
  ShieldAlert,
  Trash2,
  Undo2,
} from "lucide-react";
import { api, errorText, type Children, type Crumb, type NodeDetails, type NodeInfo, type Summary, type TreemapNode } from "../api";
import { bytes, count, date, percent, plural } from "../format";
import { ContextMenu, type MenuState } from "./ContextMenu";
import { binName, useDelete } from "./DeleteFlow";
import { Treemap, viz } from "./Treemap";
import { useToasts } from "./Toasts";

const ROW = 52;

function subtitle(n: NodeInfo): string {
  switch (n.kind) {
    case "dir":
      return n.denied ? "Couldn't open: no permission" : plural(n.files, "file");
    case "link":
      return n.mount ? "Another drive mounted here · not counted" : "Shortcut link · not followed or counted";
    case "gap":
      return "Used space with no file to show";
    case "gapPart":
      return "Managed by the system";
    default:
      return n.hardlink ? "Hard link · already counted once" : `Modified ${date(n.mtime).toLowerCase()}`;
  }
}

function KindIcon({ n }: { n: NodeInfo }) {
  if (n.kind === "dir") return n.denied ? <FolderLock /> : <Folder />;
  if (n.kind === "link") return <Link2 />;
  if (n.kind === "gap" || n.kind === "gapPart") return <HardDrive />;
  return <File />;
}

export function Explorer({
  summary,
  currentId,
  setCurrentId,
}: {
  summary: Summary;
  currentId: number;
  setCurrentId: (id: number) => void;
}) {
  const toasts = useToasts();
  const del = useDelete();
  const [kids, setKids] = useState<Children | null>(null);
  const [tm, setTm] = useState<TreemapNode | null>(null);
  const [crumbs, setCrumbs] = useState<Crumb[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [focus, setFocus] = useState(0);
  const [anchor, setAnchor] = useState(0);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [selDetails, setSelDetails] = useState<NodeDetails[]>([]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const gen = summary.generation;

  useEffect(() => {
    let alive = true;
    Promise.all([api.children(currentId), api.treemap(currentId, 3), api.node(currentId)])
      .then(([c, t, n]) => {
        if (!alive) return;
        setKids(c);
        setTm(t);
        setCrumbs(n.crumbs);
      })
      .catch((e) => {
        if (!alive) return;
        // The folder may have been deleted: go back up to the root.
        if (currentId !== 0) setCurrentId(0);
        else toasts.err(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [currentId, gen, setCurrentId, toasts]);

  // Reset selection when moving to another folder.
  useEffect(() => {
    setSelected(new Set());
    setFocus(0);
    setAnchor(0);
    scrollRef.current?.scrollTo({ top: 0 });
  }, [currentId]);

  // Drop selected ids that disappeared after a delete.
  useEffect(() => {
    if (!kids) return;
    const live = new Set(kids.items.map((i) => i.id));
    setSelected((s) => {
      const next = new Set([...s].filter((id) => live.has(id)));
      return next.size === s.size ? s : next;
    });
  }, [kids]);

  useEffect(() => {
    const ids = [...selected].slice(0, 200);
    if (!ids.length) {
      setSelDetails([]);
      return;
    }
    let alive = true;
    Promise.all(ids.map((id) => api.node(id)))
      .then((d) => alive && setSelDetails(d))
      .catch(() => alive && setSelDetails([]));
    return () => {
      alive = false;
    };
  }, [selected, gen]);

  const items = useMemo(() => kids?.items ?? [], [kids]);
  const parentSize = kids?.parent.size ?? 0;

  const virt = useVirtualizer({
    count: items.length + (kids?.rest ? 1 : 0),
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW,
    overscan: 12,
  });

  const open = useCallback(
    (id: number) => {
      setMenu(null);
      setCurrentId(id);
    },
    [setCurrentId],
  );

  const goUp = () => {
    if (crumbs.length > 1) open(crumbs[crumbs.length - 2].id);
  };

  const selectIndex = (i: number, e?: { ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean }) => {
    const item = items[i];
    if (!item) return;
    setFocus(i);
    if (e?.shiftKey) {
      const [a, b] = [Math.min(anchor, i), Math.max(anchor, i)];
      setSelected(new Set(items.slice(a, b + 1).map((x) => x.id)));
      return;
    }
    setAnchor(i);
    if (e?.ctrlKey || e?.metaKey) {
      setSelected((s) => {
        const n = new Set(s);
        if (n.has(item.id)) n.delete(item.id);
        else n.add(item.id);
        return n;
      });
    } else {
      setSelected(new Set([item.id]));
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const i = Math.max(0, Math.min(items.length - 1, focus + (e.key === "ArrowDown" ? 1 : -1)));
      selectIndex(i, { shiftKey: e.shiftKey });
      virt.scrollToIndex(i);
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      const i = e.key === "Home" ? 0 : items.length - 1;
      selectIndex(i);
      virt.scrollToIndex(i);
    } else if (e.key === "Enter" || (e.key === "ArrowRight" && !e.altKey)) {
      const it = items[focus];
      if (it?.kind === "dir") open(it.id);
    } else if (e.key === "Backspace" || e.key === "ArrowLeft" || (e.altKey && e.key === "ArrowUp")) {
      e.preventDefault();
      goUp();
    } else if (e.key === "Delete") {
      e.preventDefault();
      if (selected.size) del.remove([...selected], e.shiftKey ? "permanent" : "trash");
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
      e.preventDefault();
      setSelected(new Set(items.filter((x) => x.kind === "file" || x.kind === "dir").map((x) => x.id)));
    } else if (e.key === "Escape") {
      setSelected(new Set());
    }
  };

  const selItems = items.filter((i) => selected.has(i.id));
  const selSize = selItems.reduce((s, i) => s + i.size, 0);
  const blocked = selDetails.filter((d) => d.blocked);
  const deletable = selDetails.filter((d) => !d.blocked);
  const pseudoSel = selDetails.find((d) => d.pseudo);

  return (
    <>
      <nav className="crumbs" aria-label="Breadcrumbs">
        {crumbs.map((c, i) => (
          <span key={c.id} style={{ display: "inline-flex", alignItems: "center" }}>
            {i > 0 && <ChevronRight size={14} className="crumb-sep" />}
            <button
              className={`crumb ${i === crumbs.length - 1 ? "current" : ""}`}
              onClick={() => open(c.id)}
              aria-current={i === crumbs.length - 1 ? "page" : undefined}
              title={c.name}
              style={i === 0 ? { maxWidth: 440 } : undefined}
            >
              {c.name}
            </button>
          </span>
        ))}
      </nav>
      <div className="explorer">
        <div className="map-wrap">
          <Treemap
            data={tm}
            selected={selected}
            onOpen={open}
            onSelect={(id, additive) => {
              const i = items.findIndex((x) => x.id === id);
              if (i >= 0) {
                selectIndex(i, { ctrlKey: additive });
                virt.scrollToIndex(i);
              } else {
                setSelected(new Set([id]));
              }
            }}
            onContext={(id, x, y) => setMenu({ id, x, y })}
          />
        </div>
        <aside className="list-panel" aria-label="Items in this folder">
          <div className="list-head">
            <span className="total">{bytes(parentSize)}</span>
            <span className="muted">
              in {plural(items.length + (kids?.rest?.count ?? 0), "item")} · {count(kids?.parent.files ?? 0)} files
            </span>
          </div>
          <div
            className="list-scroll"
            ref={scrollRef}
            tabIndex={0}
            onKeyDown={onKey}
            role="listbox"
            aria-multiselectable="true"
            aria-label="Folder contents, largest first"
            aria-activedescendant={items[focus] ? `row-${items[focus].id}` : undefined}
          >
            <div style={{ height: virt.getTotalSize(), position: "relative" }}>
              {virt.getVirtualItems().map((v) => {
                if (v.index >= items.length) {
                  return (
                    <div key="rest" className="row" style={{ top: v.start, height: ROW }}>
                      <div className="rmain">
                        <div className="rname muted">{plural(kids!.rest!.count, "more small item")}</div>
                      </div>
                      <div className="rsize">{bytes(kids!.rest!.size)}</div>
                    </div>
                  );
                }
                const n = items[v.index];
                const fill = viz(v.index);
                const deep = viz(v.index, true);
                const isSel = selected.has(n.id);
                return (
                  <div
                    key={n.id}
                    id={`row-${n.id}`}
                    role="option"
                    aria-selected={isSel}
                    className={`row ${isSel ? "selected" : ""} ${v.index === focus ? "focused" : ""}`}
                    style={{ top: v.start, height: ROW }}
                    onClick={(e) => {
                      selectIndex(v.index, e);
                      scrollRef.current?.focus();
                    }}
                    onDoubleClick={() => n.kind === "dir" && open(n.id)}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      if (!selected.has(n.id)) selectIndex(v.index);
                      setMenu({ id: n.id, x: e.clientX, y: e.clientY });
                    }}
                  >
                    <div
                      className="ricon"
                      style={{
                        background: n.kind === "gap" ? "var(--sunken)" : fill,
                        color: "var(--viz-label)",
                      }}
                    >
                      <KindIcon n={n} />
                    </div>
                    <div className="rmain">
                      <div className="rname" title={n.name}>
                        {n.name}
                      </div>
                      <div className="rsub">
                        <div className="rbar" aria-hidden>
                          <span style={{ width: percent(n.size, parentSize), background: deep }} />
                        </div>
                        <span className="ellip">{subtitle(n)}</span>
                      </div>
                    </div>
                    <div>
                      <div className="rsize">{bytes(n.size)}</div>
                      <div className="rpct">{percent(n.size, parentSize)}</div>
                    </div>
                  </div>
                );
              })}
            </div>
            {kids && items.length === 0 && (
              <div className="empty">
                <Folder />
                <div>{kids.parent.denied ? "Roomy wasn't allowed to open this folder." : "This folder is empty."}</div>
              </div>
            )}
          </div>
          <div className="action-bar">
            {selected.size === 0 ? (
              <div className="sel faint">
                <span>
                  Click a block to open it. <kbd>Del</kbd> moves to the {binName}, <kbd>⌫</kbd> goes up.
                </span>
              </div>
            ) : pseudoSel ? (
              <GapExplain summary={summary} />
            ) : (
              <>
                <div className="sel">
                  <span>
                    {selected.size === 1 ? selItems[0]?.name : `${selected.size} items selected`}
                  </span>
                  <b>{bytes(selSize)}</b>
                </div>
                {blocked.length > 0 && (
                  <div className="blocked-note">
                    <ShieldAlert />
                    <span>
                      {blocked.length === selDetails.length
                        ? blocked[0].blocked
                        : `${plural(blocked.length, "protected item")} will be skipped.`}
                    </span>
                  </div>
                )}
                <div className="btns">
                  {selDetails.length === 1 && !selDetails[0].pseudo && (
                    <button className="btn sm" onClick={() => del.reveal(selDetails[0].path)}>
                      <FolderSearch /> Show
                    </button>
                  )}
                  <button
                    className="btn sm"
                    disabled={!deletable.length || del.busy}
                    onClick={() => del.remove([...selected], "trash")}
                  >
                    <Undo2 /> Move to {binName}
                  </button>
                  <button
                    className="btn sm danger-ghost"
                    disabled={!deletable.length || del.busy}
                    onClick={() => del.remove([...selected], "permanent")}
                  >
                    <Trash2 /> Delete…
                  </button>
                </div>
              </>
            )}
          </div>
        </aside>
      </div>
      {menu && <ContextMenu menu={menu} onClose={() => setMenu(null)} onOpen={open} />}
    </>
  );
}

export function GapExplain({ summary }: { summary: Summary }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, fontSize: 12.5 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", fontWeight: 650 }}>
        <CircleHelp size={15} /> Why can't I see these files?
      </div>
      {summary.gapParts.map((p) => (
        <div key={p.label} className="muted">
          <b style={{ color: "var(--text)" }}>
            {p.label} · {bytes(p.bytes)}
          </b>
          <div>{p.explain}</div>
        </div>
      ))}
    </div>
  );
}
