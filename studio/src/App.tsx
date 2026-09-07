import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { DevUpdateButton } from "./app/shell/DevUpdateButton";
import { NavTabs } from "./app/shell/NavTabs";
import { ScrollArea } from "./app/shell/ScrollArea";
import { TitleBar } from "./app/shell/TitleBar";
import { ConnectView } from "./features/films/ConnectView";
import { FilmDetailView } from "./features/films/FilmDetailView";
import { FilmsView } from "./features/films/FilmsView";
import { FriendsView } from "./features/films/FriendsView";
import { HomeView } from "./features/films/HomeView";
import { RecsView } from "./features/films/RecsView";
import { StatsView } from "./features/films/StatsView";
import { SettingsView } from "./features/settings/SettingsView";
import { resolveTheme, type Accent, type Library, type Route, type Theme } from "./core/types";
import { appVersion } from "./platform/app";
import {
  getHome,
  getSession,
  invalidateDataCache,
  invalidateTasteCache,
  migrateFromLegacy,
  setSelfUsername,
  shouldNotifyEnrichCompletion,
  syncFeeds,
  tmdbEnrich,
} from "./platform/filmLibrary";
import type { AppSession, HomeViewModel, JobProgress, LibraryCoverage } from "./platform/types/film";
import { log } from "./platform/log";
import { getSetting, setSetting } from "./platform/settings";
import { checkAppUpdate, downloadAndInstallUpdate, type UpdateProgress } from "./platform/updater";
import { UpdateOverlay } from "./app/shell/UpdateOverlay";
import "./styles.css";
import "./materials.css";

const NAV: { id: Route; label: string }[] = [
  { id: "home", label: "Home" },
  { id: "films", label: "Films" },
  { id: "friends", label: "Friends" },
  { id: "stats", label: "Stats" },
  { id: "recs", label: "Taste" },
  { id: "settings", label: "Settings" },
];

type DetailSelection = {
  id: string;
  source: Route;
};

const idleUpdateProgress: UpdateProgress = {
  phase: "idle",
  label: "Ready",
  percent: null,
  version: null,
  error: null,
};

function detailBackLabel(source: Route) {
  const label = NAV.find((item) => item.id === source)?.label;
  return label ? `Back to ${label}` : "Back";
}

export default function App() {
  const [route, setRoute] = useState<Route>("home");
  const [theme, setTheme] = useState<Theme>("dark");
  const [accent, setAccent] = useState<Accent>("app");
  const [username, setUsername] = useState("");
  const [coverage, setCoverage] = useState<LibraryCoverage | null>(null);
  const [home, setHome] = useState<HomeViewModel | null>(null);
  const [status, setStatus] = useState("");
  const [version, setVersion] = useState("…");
  const [palette, setPalette] = useState(false);
  const [hydrated, setHydrated] = useState(false);
  const [selectedFilm, setSelectedFilm] = useState<DetailSelection | null>(null);
  const [session, setSession] = useState<AppSession | null>(null);
  const [libraryEpoch, setLibraryEpoch] = useState(0);
  const [job, setJob] = useState<JobProgress | null>(null);
  const [libraryQuery, setLibraryQuery] = useState("");
  const [availableUpdate, setAvailableUpdate] = useState<string | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress>(idleUpdateProgress);
  const launchHydrationStarted = useRef(false);
  const pendingLaunchEnrich = useRef(false);
  const updateCheckStarted = useRef(false);
  const detailReturnFocus = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!status) return;
    const timeout = window.setTimeout(() => setStatus(""), 6000);
    return () => window.clearTimeout(timeout);
  }, [status]);

  const refresh = useCallback(async () => {
    try {
      invalidateDataCache();
      const [s, h] = await Promise.all([getSession(), getHome()]);
      setSession(s);
      setCoverage(s.coverage);
      setHome(h);
      setLibraryEpoch((n) => n + 1);
    } catch (err) {
      log("warn", "refresh failed", err);
    }
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = resolveTheme(theme);
    document.documentElement.dataset.accent = accent;
  }, [theme, accent]);

  useEffect(() => {
    if (launchHydrationStarted.current) return;
    launchHydrationStarted.current = true;
    void (async () => {
      try {
        const [r, t, a, u, migrated, lib, v] = await Promise.all([
          getSetting<Route>("route"),
          getSetting<Theme>("theme"),
          getSetting<Accent>("accent"),
          getSetting<string>("username"),
          getSetting<boolean>("sqlite_migrated"),
          getSetting<Library>("library"),
          appVersion().catch(() => "dev"),
        ]);
        if (r && NAV.some((n) => n.id === r)) setRoute(r);
        if (t) setTheme(t);
        if (a) setAccent(a);
        setVersion(v);

        if (lib && !migrated && (lib.films?.length || lib.username)) {
          try {
            const result = await migrateFromLegacy(lib);
            if (result.status === "completed") {
              await setSetting("sqlite_migrated", true);
              setStatus(`Migrated legacy library · ${result.validationResult}`);
            }
          } catch (err) {
            log("warn", "legacy migration skipped", err);
          }
        }

        const loadedSession = await getSession();
        setSession(loadedSession);
        setCoverage(loadedSession.coverage);
        const resolvedUsername = loadedSession.selfUsername ?? u ?? "";
        if (resolvedUsername) setUsername(resolvedUsername);
        if (u && !loadedSession.selfUsername) {
          await setSelfUsername(u);
        } else if (loadedSession.selfUsername && loadedSession.selfUsername !== u) {
          await setSetting("username", loadedSession.selfUsername);
        }

        try {
          const h = await getHome();
          setHome(h);
        } catch (err) {
          log("warn", "home load failed", err);
        }

        void syncFeeds(false)
          .then((started) => {
            if (!started) {
              void tmdbEnrich().catch((err) => log("warn", "poster enrich skipped", err));
            } else {
              pendingLaunchEnrich.current = true;
            }
          })
          .catch((err) => {
            log("warn", "diary rss sync skipped", err);
            void tmdbEnrich().catch((enrichErr) => log("warn", "poster enrich skipped", enrichErr));
          });

        setHydrated(true);
        log("info", "shell hydrated");
      } catch (err) {
        log("error", "hydrate failed", err);
        setHydrated(true);
      }
    })();
  }, [refresh]);

  useEffect(() => {
    if (!hydrated) return;
    void setSetting("route", route);
    void setSetting("theme", theme);
    void setSetting("accent", accent);
    void setSetting("username", username);
    void setSelfUsername(username).catch((err) => log("warn", "username persist failed", err));
  }, [route, theme, accent, username, hydrated]);

  useEffect(() => {
    if (!hydrated || import.meta.env.DEV || updateCheckStarted.current) return;
    updateCheckStarted.current = true;
    void checkAppUpdate().then((result) => {
      if (result.available && result.version && result.signingConfigured) {
        setAvailableUpdate(result.version);
        setUpdateDismissed(false);
      }
    });
  }, [hydrated]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<JobProgress>("studio-job", (event) => {
      const next = event.payload;
      setJob(next.done ? null : next);
      if (next.done && (next.job !== "enrich" || (next.enrich && shouldNotifyEnrichCompletion(next.enrich)))) {
        setStatus(next.label);
      }
      if (next.done) {
        if (next.job === "taste") {
          invalidateTasteCache();
        } else {
          void refresh();
        }
      }
      if (next.done && next.job === "feeds") {
        const added = next.feeds?.entriesAdded ?? 0;
        if (pendingLaunchEnrich.current || added > 0) {
          pendingLaunchEnrich.current = false;
          void tmdbEnrich().catch((err) => log("warn", "poster enrich after diary sync skipped", err));
        }
      }
      if (next.done && next.job === "sync") {
        void tmdbEnrich().catch((err) => log("warn", "poster enrich after diary sync skipped", err));
      }
      if (next.done && next.import) {
        void tmdbEnrich().catch((err) => log("warn", "poster enrich after import skipped", err));
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [refresh]);

  useEffect(() => {
    function onFocus() {
      void syncFeeds(false).catch((err) => log("warn", "diary rss sync skipped", err));
    }
    window.addEventListener("focus", onFocus);
    const timer = window.setInterval(() => void refresh(), 60 * 60 * 1000);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.clearInterval(timer);
    };
  }, [refresh]);

  const closePalette = useCallback(() => setPalette(false), []);

  const installAvailableUpdate = useCallback(async () => {
    setUpdateOpen(true);
    setUpdateProgress({
      phase: "checking",
      label: "Preparing update…",
      percent: null,
      version: availableUpdate,
      error: null,
    });
    await downloadAndInstallUpdate(setUpdateProgress);
  }, [availableUpdate]);

  const closeDetail = useCallback(() => {
    setSelectedFilm(null);
    const focusTarget = detailReturnFocus.current;
    detailReturnFocus.current = null;
    window.requestAnimationFrame(() => focusTarget?.focus());
  }, []);

  const openFilm = useCallback((id: string, source: Route) => {
    detailReturnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSelectedFilm({ id, source });
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        if (selectedFilm) {
          closeDetail();
          return;
        }
        if (palette) {
          e.preventDefault();
          closePalette();
        }
      }
      const meta = e.ctrlKey || e.metaKey;
      if (meta && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette(true);
      }
      if (meta && e.key === ",") {
        e.preventDefault();
        setRoute("settings");
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, closePalette, selectedFilm, closeDetail]);

  const connected = Boolean(session?.hasSetup);

  function go(id: Route) {
    closeDetail();
    setRoute(id);
  }

  function onNavSearch(value: string) {
    setLibraryQuery(value);
    if (route !== "films") {
      closeDetail();
      setRoute("films");
    }
  }

  return (
    <div className="app canvas-surface">
      <TitleBar />
      <ScrollArea scrollKey={selectedFilm ? `film:${selectedFilm.id}` : route === "films" ? `films:${libraryQuery}` : route}>
        <header className="cinema-nav">
          <div className="cinema-nav-inner">
            <NavTabs items={NAV} active={route} onGo={go} />
            <label className="nav-search glass">
              <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5" /><path d="m16 16 4.5 4.5" /></svg>
              <input
                type="search"
                aria-label="Search your log"
                value={libraryQuery}
                onChange={(e) => onNavSearch(e.target.value)}
                placeholder="Search your log"
                spellCheck={false}
              />
            </label>
            <div className="nav-utility"><DevUpdateButton /></div>
          </div>
        </header>
        <main className="main-shell">
          <div className={`main-content${!connected && route === "home" ? " is-connect" : ""}`}>
            {!connected && route === "home" ? (
              <ConnectView
                username={username}
                onUsername={setUsername}
                onStatus={setStatus}
                onConnected={refresh}
              />
            ) : selectedFilm ? (
              <FilmDetailView
                filmId={selectedFilm.id}
                onBack={closeDetail}
                backLabel={detailBackLabel(selectedFilm.source)}
                onStatus={setStatus}
                onSelectFilm={(id) => openFilm(id, selectedFilm.source)}
                onArtworkChange={refresh}
              />
            ) : (
              <>
                {route === "home" ? (
                  <HomeView
                    home={home}
                    onOpenFilms={() => setRoute("films")}
                    onOpenFriends={() => setRoute("friends")}
                    onSelectFilm={(id) => openFilm(id, "home")}
                  />
                ) : null}
                {route === "films" ? (
                  <FilmsView
                    onSelectFilm={(id) => openFilm(id, "films")}
                    onStatus={setStatus}
                    reloadToken={libraryEpoch}
                    query={libraryQuery}
                  />
                ) : null}
                {route === "friends" ? (
                  <FriendsView onStatus={setStatus} onRefresh={refresh} />
                ) : null}
                {route === "stats" ? <StatsView onSelectFilm={(id) => openFilm(id, "stats")} /> : null}
                {route === "recs" ? (
                  <RecsView onSelectFilm={(id) => openFilm(id, "recs")} onOpenSettings={() => setRoute("settings")} />
                ) : null}
                {route === "settings" ? (
                  <SettingsView
                    theme={theme}
                    accent={accent}
                    version={version}
                    username={username}
                    coverage={coverage}
                    lastRssSyncAt={session?.lastRssSyncAt}
                    rssPausedUntil={session?.rssPausedUntil}
                    onTheme={setTheme}
                    onAccent={setAccent}
                    onUsername={setUsername}
                    onStatus={setStatus}
                    onRefresh={refresh}
                  />
                ) : null}
              </>
            )}
          </div>
        </main>
      </ScrollArea>
      {job || status || (availableUpdate && !updateDismissed) ? (
        <div className="toast-stack">
          {job || status ? (
            <aside className={`activity-toast${job ? " is-busy" : ""}`} role="status" aria-live="polite" aria-atomic="true">
              <span className="activity-indicator" aria-hidden="true" />
              <div className="activity-copy">
                <strong>{job?.label ?? status}</strong>
                {job ? (
                  <>
                    {job.total > 0 || job.posters ? (
                      <span>
                        {job.total > 0 ? `${job.current}/${job.total}` : null}
                        {job.posters ? ` · ${job.posters} posters` : ""}
                      </span>
                    ) : null}
                    {job.total > 0 ? (
                      <div className="activity-progress" aria-hidden="true">
                        <span style={{ width: `${Math.min(100, Math.round((job.current / job.total) * 100))}%` }} />
                      </div>
                    ) : null}
                  </>
                ) : null}
              </div>
              {!job ? (
                <button className="activity-dismiss" type="button" aria-label="Dismiss status" onClick={() => setStatus("")}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m7 7 10 10M17 7 7 17" /></svg>
                </button>
              ) : null}
            </aside>
          ) : null}
          {availableUpdate && !updateDismissed ? (
            <aside className="activity-toast update-toast" role="status" aria-live="polite" aria-atomic="true">
              <span className="update-available-icon" aria-hidden="true">
                <svg viewBox="0 0 24 24"><path d="M12 4v11m0 0 4-4m-4 4-4-4M5 20h14" /></svg>
              </span>
              <div className="activity-copy">
                <strong>Osvell {availableUpdate} is ready</strong>
                <span>Install the latest improvements.</span>
              </div>
              <button className="update-toast-action" type="button" onClick={() => void installAvailableUpdate()}>
                Update now
              </button>
              <button className="activity-dismiss" type="button" aria-label="Dismiss update notice" onClick={() => setUpdateDismissed(true)}>
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m7 7 10 10M17 7 7 17" /></svg>
              </button>
            </aside>
          ) : null}
        </div>
      ) : null}
      <UpdateOverlay
        open={updateOpen}
        title="Installing update"
        progress={updateProgress}
        onClose={() => {
          setUpdateOpen(false);
          setUpdateProgress(idleUpdateProgress);
        }}
      />
      {palette ? (
        <div className="overlay" onMouseDown={closePalette}>
          <div className="palette glass" onMouseDown={(e) => e.stopPropagation()}>
            <input autoFocus placeholder="Search Osvell" readOnly />
            {NAV.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => {
                  go(item.id);
                  closePalette();
                }}
              >
                Go to {item.label}
              </button>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
