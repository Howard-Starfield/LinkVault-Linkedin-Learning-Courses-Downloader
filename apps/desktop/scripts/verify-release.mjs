import { spawnSync } from "node:child_process";

const commands = [
  ["node", ["./scripts/verify-release-manifest.mjs"]],
  ["node", ["./scripts/verify-ui.mjs"]],
  ["npm.cmd", ["run", "build"]],
  ["cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib"]]
];

for (const [command, args] of commands) {
  const result = spawnSync(command, args, {
    cwd: new URL("..", import.meta.url),
    encoding: "utf8",
    shell: process.platform === "win32",
    stdio: "inherit",
    env: { ...process.env, CARGO_BUILD_JOBS: "1" }
  });
  if (result.status !== 0) process.exit(result.status ?? 1);
}
console.log("Release verification passed.");
