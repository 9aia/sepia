import { jsonResponse, segmentsEqual, type RouteHandler } from "./shared";

/**
 * `POST /api/client/keypair` — client keypair mint: a client on a
 * non-secure context (http:// over LAN) has no crypto.subtle, so it can't
 * generate its Ed25519/ECDSA identity; the node mints one server-side.
 * Authenticated like every other /api route — the secret transits the
 * wire, so on plaintext LAN this is only as private as the transport.
 */
export const createClientRoute =
  (): RouteHandler =>
  async ({ method, segments, cors }) => {
    if (method === "POST" && segmentsEqual(segments, ["api", "client", "keypair"])) {
      const subtle = crypto.subtle;
      const attempts: ReadonlyArray<{
        params: { name: string; namedCurve?: string };
        algorithm: string;
      }> = [
        { params: { name: "Ed25519" }, algorithm: "Ed25519" },
        { params: { name: "ECDSA", namedCurve: "P-256" }, algorithm: "ECDSA-P-256" },
      ];
      const b64url = (buf: ArrayBuffer): string =>
        Buffer.from(buf)
          .toString("base64")
          .replace(/\+/g, "-")
          .replace(/\//g, "_")
          .replace(/=+$/, "");
      for (const { params, algorithm } of attempts) {
        try {
          const pair = (await subtle.generateKey(params, true, ["sign", "verify"])) as {
            publicKey: CryptoKey;
            privateKey: CryptoKey;
          };
          const raw = await subtle.exportKey("raw", pair.publicKey);
          const jwk = await subtle.exportKey("jwk", pair.privateKey);
          if (typeof jwk.d !== "string" || jwk.d === "") continue;
          return jsonResponse({ algorithm, publicKey: b64url(raw), secretKey: jwk.d }, 200, cors);
        } catch {
          // Algorithm unsupported in this runtime — try the next.
        }
      }
      return jsonResponse({ error: "No supported key algorithm in this runtime" }, 501, cors);
    }
    return undefined;
  };
