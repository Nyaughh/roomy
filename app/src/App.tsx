import { useCallback, useEffect, useState } from "react";
import { api, errorText, onScanDone, type AppInfo, type Summary } from "./api";
import { Home } from "./components/Home";
import { Results } from "./components/Results";
import { Scanning } from "./components/Scanning";
import { ToastProvider, useToasts } from "./components/Toasts";

type Screen = { kind: "home" } | { kind: "scanning"; path: string } | { kind: "results" };

function Main() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [screen, setScreen] = useState<Screen>({ kind: "home" });
  const [summary, setSummary] = useState<Summary | null>(null);
  // Bumped per scan so result views start fresh (node ids change).
  const [scanKey, setScanKey] = useState(0);
  const toasts = useToasts();

  useEffect(() => {
    api.appInfo().then(setInfo, () => {});
    // Restore an existing scan (e.g. after a dev reload).
    api.summary().then((s) => {
      if (s) {
        setSummary(s);
        setScreen({ kind: "results" });
      }
    });
  }, []);

  useEffect(() => {
    const un = onScanDone((d) => {
      if (d.ok && d.summary) {
        setSummary(d.summary);
        setScanKey((k) => k + 1);
        setScreen({ kind: "results" });
      } else if (!d.cancelled) {
        toasts.err(d.error ?? "The scan didn't finish.");
        setScreen({ kind: "home" });
      }
    });
    return () => {
      void un.then((f) => f());
    };
  }, [toasts]);

  const scan = useCallback(
    (path: string) => {
      setScreen({ kind: "scanning", path });
      api.startScan(path).catch((e) => {
        toasts.err(errorText(e));
        setScreen({ kind: "home" });
      });
    },
    [toasts],
  );

  const home = useCallback(() => {
    void api.closeScan();
    setSummary(null);
    setScreen({ kind: "home" });
  }, []);

  if (screen.kind === "scanning") {
    return <Scanning path={screen.path} onCancel={() => setScreen(summary ? { kind: "results" } : { kind: "home" })} />;
  }
  if (screen.kind === "results" && summary) {
    return (
      <Results
        key={scanKey}
        info={info}
        summary={summary}
        setSummary={setSummary}
        onRescan={() => scan(summary.rootPath)}
        onHome={home}
      />
    );
  }
  return <Home info={info} onScan={scan} />;
}

export function App() {
  return (
    <ToastProvider>
      <Main />
    </ToastProvider>
  );
}
