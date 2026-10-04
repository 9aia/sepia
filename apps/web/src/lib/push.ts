import { getToken } from "./api";

/** Notification categories the user can toggle independently. */
export interface NotificationPrefs {
  readonly done: boolean;
  readonly permission: boolean;
}

export const isPushSupported = (): boolean =>
  "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;

const urlBase64ToUint8Array = (base64: string): Uint8Array<ArrayBuffer> => {
  const padding = "=".repeat((4 - (base64.length % 4)) % 4);
  const raw = atob((base64 + padding).replace(/-/g, "+").replace(/_/g, "/"));
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
};

const headers = (): Record<string, string> => ({
  "content-type": "application/json",
  ...(getToken() !== null ? { authorization: `Bearer ${getToken()}` } : {}),
});

/** Subscribe this browser and register the subscription server-side. */
export const subscribePush = async (prefs: NotificationPrefs): Promise<boolean> => {
  if (!isPushSupported()) return false;
  const vapidRes = await fetch("/api/push/vapid", { headers: headers() });
  if (!vapidRes.ok) return false;
  const { publicKey } = (await vapidRes.json()) as { publicKey: string };
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: urlBase64ToUint8Array(publicKey) as BufferSource,
  });
  const res = await fetch("/api/push/subscribe", {
    method: "POST",
    headers: headers(),
    body: JSON.stringify({ ...subscription.toJSON(), prefs }),
  });
  return res.ok;
};

/** Re-register the same subscription with updated notification prefs. */
export const updatePushPrefs = async (prefs: NotificationPrefs): Promise<boolean> =>
  subscribePush(prefs);

/** Unregister locally + remove the server-side subscription. */
export const unsubscribePush = async (): Promise<void> => {
  if (!isPushSupported()) return;
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.getSubscription();
  if (subscription !== null) {
    await fetch("/api/push/subscribe", {
      method: "DELETE",
      headers: headers(),
      body: JSON.stringify({ endpoint: subscription.endpoint }),
    }).catch(() => {});
    await subscription.unsubscribe();
  }
};
