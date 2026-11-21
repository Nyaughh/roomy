import { useEffect, useRef, useState } from "react";
import { FolderOpen, FolderSearch, ShieldAlert, Trash2, Undo2 } from "lucide-react";
import { api, type NodeDetails } from "../api";
import { binName, useDelete } from "./DeleteFlow";

export interface MenuState {
  id: number;
  x: number;
  y: number;
}

export function ContextMenu({
  menu,
  onClose,
  onOpen,
}: {
  menu: MenuState;
  onClose: () => void;
  onOpen: (id: number) => void;
}) {
  const [d, setD] = useState<NodeDetails | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const del = useDelete();

  useEffect(() => {
    api.node(menu.id).then(setD, () => onClose());
  }, [menu.id, onClose]);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", key);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", key);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  useEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [d]);

  if (!d) return null;
  const left = Math.min(menu.x, window.innerWidth - 240);
  const top = Math.min(menu.y, window.innerHeight - 220);
  const act = (f: () => void) => () => {
    onClose();
    f();
  };

  return (
    <div className="menu" style={{ left, top }} ref={ref} role="menu">
      <div className="menu-title">{d.node.name}</div>
      {d.node.kind === "dir" && (
        <button role="menuitem" onClick={act(() => onOpen(d.node.id))}>
          <FolderOpen /> Open
        </button>
      )}
      {!d.pseudo && (
        <button role="menuitem" onClick={act(() => del.reveal(d.path))}>
          <FolderSearch /> Show in {navigator.userAgent.includes("Mac") ? "Finder" : "Explorer"}
        </button>
      )}
      <hr />
      {d.blocked ? (
        <div className="menu-title" style={{ whiteSpace: "normal", display: "flex", gap: 8 }}>
          <ShieldAlert size={14} style={{ flex: "none", marginTop: 2, color: "var(--amber)" }} />
          <span>{d.blocked}</span>
        </div>
      ) : (
        <>
          <button role="menuitem" onClick={act(() => del.remove([d.node.id], "trash"))}>
            <Undo2 /> Move to {binName}
            <kbd style={{ marginLeft: "auto" }}>Del</kbd>
          </button>
          <button role="menuitem" className="danger" onClick={act(() => del.remove([d.node.id], "permanent"))}>
            <Trash2 /> Delete permanently…
          </button>
        </>
      )}
    </div>
  );
}
