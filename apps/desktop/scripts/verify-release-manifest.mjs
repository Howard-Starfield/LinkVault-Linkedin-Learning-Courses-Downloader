import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
const tauriConfig = JSON.parse(await readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const cargo = await readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8");
const app = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
const cargoVersion = cargo.match(/^version = "([^"]+)"/m)?.[1];
const appVersion = app.match(/const APP_VERSION = "([^"]+)"/)?.[1];

assert.ok(cargoVersion, "Cargo version was not found.");
assert.ok(appVersion, "App version was not found.");
assert.equal(packageJson.version, cargoVersion, "package.json and Cargo.toml versions differ.");
assert.equal(packageJson.version, tauriConfig.version, "package.json and tauri.conf.json versions differ.");
assert.equal(packageJson.version, appVersion, "package.json and App.tsx versions differ.");
console.log(`Release manifest verification passed for ${packageJson.version}.`);
