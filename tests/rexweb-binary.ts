import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Resolve overrides before a test changes the service's working directory.
export function rexwebBinary(path = process.env.REXWEB_BIN): string {
  return path ? resolve(path) : fileURLToPath(new URL("../bin/rexweb", import.meta.url));
}
