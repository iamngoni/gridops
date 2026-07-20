import "@tanstack/react-start/server-only";

import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
  timingSafeEqual,
} from "node:crypto";

import { getConfig } from "./config.server";

function decodeKey(value: string | undefined, name: string) {
  if (!value) {
    throw new Error(`${name} is required for secure credential storage.`);
  }

  const key = Buffer.from(value, "base64");
  if (key.length !== 32) {
    throw new Error(`${name} must be a base64-encoded 32-byte value.`);
  }

  return key;
}

export function randomToken(bytes = 32) {
  return randomBytes(bytes).toString("base64url");
}

export function hashToken(value: string) {
  return createHash("sha256").update(value).digest("base64url");
}

export function seal(value: string) {
  const key = decodeKey(getConfig().encryptionKey, "GRIDOPS_ENCRYPTION_KEY");
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  const ciphertext = Buffer.concat([cipher.update(value, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag();

  return [iv, tag, ciphertext].map((part) => part.toString("base64url")).join(".");
}

export function unseal(value: string) {
  const key = decodeKey(getConfig().encryptionKey, "GRIDOPS_ENCRYPTION_KEY");
  const [encodedIv, encodedTag, encodedCiphertext] = value.split(".");
  if (!encodedIv || !encodedTag || !encodedCiphertext) {
    throw new Error("Encrypted value has an invalid format.");
  }

  const decipher = createDecipheriv(
    "aes-256-gcm",
    key,
    Buffer.from(encodedIv, "base64url"),
  );
  decipher.setAuthTag(Buffer.from(encodedTag, "base64url"));

  return Buffer.concat([
    decipher.update(Buffer.from(encodedCiphertext, "base64url")),
    decipher.final(),
  ]).toString("utf8");
}

export function signValue(value: string) {
  const secret = getConfig().sessionSecret;
  if (!secret) {
    throw new Error("GRIDOPS_SESSION_SECRET is required for authenticated sessions.");
  }

  return createHmac("sha256", secret).update(value).digest("base64url");
}

export function verifySignature(value: string, signature: string) {
  const expected = Buffer.from(signValue(value));
  const actual = Buffer.from(signature);
  return expected.length === actual.length && timingSafeEqual(expected, actual);
}
