import { useEffect, useState } from "react";
import { loadResidentPrefs, subscribeResident } from "../../platform/resident";
import { windowApi } from "../../platform/window";

export function TitleBar() {
  const [maximized, setMaximized] = useState(false);
  const [closeMinimizes, setCloseMinimizes] = useState(false);

  useEffect(() => {
    const win = windowApi();
    let unlisten: (() => void) | undefined;
    void win.isMaximized().then(setMaximized);
    void win.onResized(async () => setMaximized(await win.isMaximized())).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    let alive = true;
    void loadResidentPrefs().then((prefs) => {
      if (alive) setCloseMinimizes(prefs.closeMinimizes);
    });
    const unsubscribe = subscribeResident((prefs) => setCloseMinimizes(prefs.closeMinimizes));
    return () => {
      alive = false;
      unsubscribe();
    };
  }, []);

  const win = windowApi();
  const closeLabel = closeMinimizes ? "Hide" : "Close";

  return (
    <header className="titlebar">
      <div className="tb-drag" data-tauri-drag-region onDoubleClick={() => void win.toggleMaximize()} />
      <div className="tb-controls glass" role="toolbar" aria-label="Window">
        <button type="button" className="tb-btn" aria-label="Minimize" title="Minimize" onClick={() => void win.minimize()}>
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path fill="currentColor" d="M2.5 5.75h7v.9h-7z" />
          </svg>
        </button>
        <button type="button" className="tb-btn" aria-label={maximized ? "Restore" : "Maximize"} title={maximized ? "Restore" : "Maximize"} onClick={() => void win.toggleMaximize()}>
          {maximized ? (
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
              <path fill="none" stroke="currentColor" strokeWidth="1.2" d="M3.4 4.2h5.2v5.2H3.4zM4.4 4.2V2.8h5.2v5.2H8.2" />
            </svg>
          ) : (
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
              <rect x="2.6" y="2.6" width="6.8" height="6.8" fill="none" stroke="currentColor" strokeWidth="1.2" rx="0.6" />
            </svg>
          )}
        </button>
        <button
          type="button"
          className="tb-btn close"
          aria-label={closeLabel}
          title={closeMinimizes ? "Hide. Shift-click to quit" : "Close"}
          onClick={(event) => {
            if (closeMinimizes && event.shiftKey) {
              void win.quit();
              return;
            }
            void win.close();
          }}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path
              fill="currentColor"
              d="M3.1 2.4 6 5.3l2.9-2.9.7.7L6.7 6l2.9 2.9-.7.7L6 6.7 3.1 9.6l-.7-.7L5.3 6 2.4 3.1z"
            />
          </svg>
        </button>
      </div>
    </header>
  );
}
