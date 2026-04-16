import { randomBytes, scryptSync, timingSafeEqual } from "node:crypto";
import process from "node:process";

const password = process.argv[2] ?? process.env.CODEX_WEBUI_PASSWORD ?? "";

if (!password) {
  console.error("Usage: pnpm hash-password -- <password>");
  process.exit(1);
}

const salt = randomBytes(16);
const key = scryptSync(password, salt, 64);
const hash = `scrypt$${salt.toString("base64url")}$${key.toString("base64url")}`;

if (process.argv.includes("--verify")) {
  const candidate = process.argv[3];
  if (!candidate) {
    console.error("Usage: pnpm hash-password -- --verify <hash>");
    process.exit(1);
  }
  const [, savedSalt, savedKey] = candidate.split("$");
  const derived = scryptSync(password, Buffer.from(savedSalt, "base64url"), 64);
  const valid = timingSafeEqual(derived, Buffer.from(savedKey, "base64url"));
  console.log(valid ? "valid" : "invalid");
  process.exit(valid ? 0 : 1);
}

console.log(hash);
