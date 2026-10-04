import webpush from "web-push";
import type { MetaStore } from "./meta";

/** Notification categories a subscription opts into/out of. */
export interface PushPrefs {
  /** Agent run finished / errored. */
  readonly done: boolean;
  /** Agent is waiting for a permission decision. */
  readonly permission: boolean;
}

export interface PushSubscription {
  readonly endpoint: string;
  readonly keys: { readonly auth: string; readonly p256dh: string };
  readonly prefs: PushPrefs;
}

export interface PushStore {
  readonly publicKey: string;
  readonly list: () => ReadonlyArray<PushSubscription>;
  readonly upsert: (sub: PushSubscription) => void;
  readonly remove: (endpoint: string) => void;
  readonly send: (
    kind: keyof PushPrefs,
    title: string,
    body: string,
    url: string,
  ) => Promise<number>;
}

const DEFAULT_PREFS: PushPrefs = { done: true, permission: true };

interface StoredSub {
  readonly endpoint: string;
  readonly keys: { auth: string; p256dh: string };
  readonly prefs?: Partial<PushPrefs>;
}

const readSubs = (meta: MetaStore): PushSubscription[] => {
  const raw = (meta.config() as { pushSubscriptions?: StoredSub[] }).pushSubscriptions ?? [];
  return raw.map((sub) => ({
    endpoint: sub.endpoint,
    keys: { auth: sub.keys.auth, p256dh: sub.keys.p256dh },
    prefs: { ...DEFAULT_PREFS, ...sub.prefs },
  }));
};

const writeSubs = (meta: MetaStore, subs: ReadonlyArray<PushSubscription>): void => {
  meta.setConfig("pushSubscriptions", subs);
};

/**
 * Web Push over the meta store — VAPID keys generate once and persist next to
 * sessions/projects; subscriptions live under the `pushSubscriptions` key.
 */
export const makePushStore = (meta: MetaStore): PushStore => {
  // Keys persist in config so restarts keep existing subscriptions valid.
  const vapid = (meta.config() as { vapid?: { publicKey: string; privateKey: string } }).vapid;
  const keys =
    vapid ??
    (() => {
      const generated = webpush.generateVAPIDKeys();
      meta.setConfig("vapid", generated);
      return generated;
    })();
  webpush.setVapidDetails("mailto:sepia@localhost", keys.publicKey, keys.privateKey);

  const upsert = (sub: PushSubscription): void => {
    const subs = readSubs(meta).filter((s) => s.endpoint !== sub.endpoint);
    subs.push(sub);
    writeSubs(meta, subs);
  };

  const send = async (
    kind: keyof PushPrefs,
    title: string,
    body: string,
    url: string,
  ): Promise<number> => {
    const targets = readSubs(meta).filter((sub) => sub.prefs[kind]);
    const results = await Promise.allSettled(
      targets.map((sub) =>
        webpush.sendNotification(
          { endpoint: sub.endpoint, keys: sub.keys },
          JSON.stringify({ title, body, url, tag: `sepia-${kind}` }),
          { TTL: 300 },
        ),
      ),
    );
    // Expired subscriptions (404/410) are pruned.
    for (const [index, result] of results.entries()) {
      if (result.status === "rejected") {
        const code = (result.reason as { statusCode?: number }).statusCode;
        if (code === 404 || code === 410) {
          writeSubs(
            meta,
            readSubs(meta).filter((s) => s.endpoint !== targets[index]?.endpoint),
          );
        }
      }
    }
    return results.filter((r) => r.status === "fulfilled").length;
  };

  return {
    publicKey: keys.publicKey,
    list: () => readSubs(meta),
    upsert,
    remove: (endpoint) =>
      writeSubs(
        meta,
        readSubs(meta).filter((s) => s.endpoint !== endpoint),
      ),
    send,
  };
};

/** Session-event → notification text mapping (RUN_FINISHED etc.). */
export const notifyForEvents = (
  push: PushStore,
  sessionId: string,
  agent: string | undefined,
  sessionTitle: string,
  events: ReadonlyArray<{ type?: string; name?: string }>,
): void => {
  const key = agent === undefined ? sessionId : `${agent}:${sessionId}`;
  const url = `/?session=${encodeURIComponent(key)}`;
  if (events.some((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR")) {
    void push.send("done", "Session finished", `${sessionTitle} finished its run.`, url);
  }
  if (events.some((e) => e.type === "CUSTOM" && e.name === "acp:permission_request")) {
    void push.send("permission", "Approval needed", `${sessionTitle} is waiting for you.`, url);
  }
};
