import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";
import { CircleAlert, CircleCheck } from "lucide-react";

interface Toast {
  id: number;
  kind: "ok" | "err";
  text: string;
}

interface ToastApi {
  ok: (text: string) => void;
  err: (text: string) => void;
}

const Ctx = createContext<ToastApi>({ ok: () => {}, err: () => {} });

export function useToasts() {
  return useContext(Ctx);
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(1);
  const push = useCallback((kind: Toast["kind"], text: string) => {
    const id = next.current++;
    setToasts((t) => [...t.slice(-3), { id, kind, text }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "err" ? 8000 : 5000);
  }, []);
  const api = useMemo(() => ({ ok: (t: string) => push("ok", t), err: (t: string) => push("err", t) }), [push]);
  return (
    <Ctx.Provider value={api}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`}>
            {t.kind === "ok" ? <CircleCheck /> : <CircleAlert />}
            <span>{t.text}</span>
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}
