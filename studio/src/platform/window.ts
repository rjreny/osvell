import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

function hostWindow() {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: unknown } } })
    .__TAURI_INTERNALS__;
  if (!internals?.metadata?.currentWindow) return null;
  return getCurrentWindow();
}

export function windowApi() {
  const win = hostWindow();
  return {
    minimize: () => win?.minimize() ?? Promise.resolve(),
    toggleMaximize: () => win?.toggleMaximize() ?? Promise.resolve(),
    close: () => win?.close() ?? Promise.resolve(),
    quit: () => (win ? invoke("quit_app") : Promise.resolve()),
    isMaximized: () => win?.isMaximized() ?? Promise.resolve(false),
    onResized: (cb: () => void) => win?.onResized(cb) ?? Promise.resolve(() => undefined),
  };
}
