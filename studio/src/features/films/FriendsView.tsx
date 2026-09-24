import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  getHome,
  importFriendUsernames,
  listFriends,
  removeFriend,
  syncFriends,
} from "../../platform/filmLibrary";
import type { FriendActivityItem, FriendRow, JobProgress } from "../../platform/types/film";
import { Poster } from "./Poster";
import { RatingDisplay } from "./RatingDisplay";

export function FriendsView({
  onStatus,
  onRefresh,
  onSelectFilm,
}: {
  onStatus: (s: string) => void;
  onRefresh: () => Promise<void>;
  onSelectFilm?: (id: string) => void;
}) {
  const [friends, setFriends] = useState<FriendRow[]>([]);
  const [draft, setDraft] = useState("");
  const [feed, setFeed] = useState<FriendActivityItem[]>([]);
  const [busy, setBusy] = useState(false);

  async function load() {
    const [rows, home] = await Promise.all([listFriends(), getHome()]);
    setFriends(rows);
    setFeed(home.friendFeed);
  }

  useEffect(() => {
    void load().catch(() => onStatus("Could not load friends"));
  }, [onStatus]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<JobProgress>("studio-job", (event) => {
      if (event.payload.done && (event.payload.job === "feeds" || event.payload.job === "friends")) {
        void load().catch(() => onStatus("Could not refresh friends"));
        void onRefresh();
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [onRefresh, onStatus]);

  async function addFriends() {
    if (!draft.trim()) return;
    setBusy(true);
    try {
      const added = await importFriendUsernames(draft);
      setDraft("");
      await syncFriends();
      await load();
      await onRefresh();
      onStatus(`Added ${added} friend${added === 1 ? "" : "s"} · refreshing their public diaries`);
    } finally {
      setBusy(false);
    }
  }

  async function removeOne(friend: FriendRow) {
    const ok = await ask(
      `Stop following @${friend.username}? Their ratings will leave your feed.`,
      { title: "Remove friend?", kind: "warning", okLabel: "Remove", cancelLabel: "Cancel" },
    );
    if (!ok) return;
    setBusy(true);
    try {
      await removeFriend(friend.id);
      await load();
      await onRefresh();
      onStatus(`Removed @${friend.username}`);
    } catch {
      onStatus(`Could not remove @${friend.username}`);
    } finally {
      setBusy(false);
    }
  }

  async function refreshAll() {
    setBusy(true);
    try {
      await syncFriends();
      onStatus("Syncing friend feeds in the background");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="friends-page page-pad">
      <div className="utility-canvas">
      <header className="page-head friends-head">
        <div>
          <h1>Friends</h1>
          <p className="muted">See what people you follow have been watching</p>
        </div>
        <button type="button" className="play-btn" disabled={busy} onClick={() => void refreshAll()}>
          {busy ? "Syncing…" : "Sync all"}
        </button>
      </header>
      <div className="friends-layout">
        <aside className="friends-side">
          <div className="friends-section-head">
            <div>
              <h2>Following</h2>
              <p>{friends.length ? `${friends.length} ${friends.length === 1 ? "friend" : "friends"}` : "No one yet"}</p>
            </div>
          </div>
          <form
            className="friend-add"
            onSubmit={(e) => {
              e.preventDefault();
              void addFriends();
            }}
          >
            <label className="sr-only" htmlFor="friend-usernames">Letterboxd usernames</label>
            <input
              id="friend-usernames"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder="Username, or several…"
              autoCapitalize="off"
              spellCheck={false}
            />
            <button type="submit" className="ghost-pill" disabled={busy || !draft.trim()}>
              Add
            </button>
          </form>
          <ul className="friend-list">
            {friends.map((f) => (
              <li key={f.id}>
                <div className="friend-list-copy">
                  <strong>@{f.username}</strong>
                  <span className="muted">
                    {f.lastSyncAt ? `Updated ${new Date(f.lastSyncAt).toLocaleDateString()}` : "Waiting for first sync"}
                  </span>
                  {f.lastSyncError ? <span className="form-error">{f.lastSyncError}</span> : null}
                </div>
                <button
                  type="button"
                  className="friend-more"
                  disabled={busy}
                  aria-label={`Remove @${f.username}`}
                  onClick={() => void removeOne(f)}
                >
                  <span aria-hidden="true">···</span>
                </button>
              </li>
            ))}
            {!friends.length ? (
              <li className="friends-empty">
                Add one or several Letterboxd usernames to bring their latest ratings into Osvell.
              </li>
            ) : null}
          </ul>
        </aside>
        <section className="friends-feed">
          <div className="friends-section-head">
            <div>
              <h2>Latest ratings</h2>
              <p>{feed.length ? `${feed.length} recent ${feed.length === 1 ? "entry" : "entries"}` : "Nothing here yet"}</p>
            </div>
          </div>
          <ul className="activity-list">
            {feed.map((e, idx) => {
              const open = e.filmId && onSelectFilm ? () => onSelectFilm(e.filmId!) : null;
              const body = (
                <>
                  <Poster name={e.title} poster={e.poster} />
                  <span className="activity-copy">
                    <strong title={e.title}>{e.title}</strong>
                    <span className="muted">
                      @{e.username}
                      {e.year ? ` · ${e.year}` : ""}
                    </span>
                    <span className="activity-rating" aria-label={`${e.rating} out of 5 stars`}>
                      <RatingDisplay value={e.rating} starsOnly />
                    </span>
                  </span>
                </>
              );
              return (
                <li key={`${e.username}-${e.title}-${idx}`}>
                  {open ? (
                    <button type="button" className="activity-card" onClick={open}>
                      {body}
                    </button>
                  ) : (
                    <div className="activity-card">{body}</div>
                  )}
                </li>
              );
            })}
          </ul>
          {!feed.length ? (
            <div className="friends-feed-empty">
              <strong>Your friends’ latest ratings will land here.</strong>
              <p>Follow a public diary, then sync to get started.</p>
            </div>
          ) : null}
        </section>
      </div>
      </div>
    </div>
  );
}
