import { spawnSync } from "node:child_process";

const result = spawnSync("cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib"], {
  cwd: new URL("..", import.meta.url),
  encoding: "utf8",
  shell: process.platform === "win32",
  stdio: "inherit",
  env: { ...process.env, CARGO_BUILD_JOBS: "1" }
});
process.exit(result.status ?? 1);
