import assert from "node:assert/strict";
import { readFile, access } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const json = async (name) => JSON.parse(await readFile(path.join(root, name), "utf8"));
const pkg = await json("package.json");
const lock = await json("package-lock.json");
const config = await json("src-tauri/tauri.conf.json");
const cargo = await readFile(path.join(root, "src-tauri/Cargo.toml"), "utf8");
const cargoPackage = cargo.split("[package]")[1]?.split(/\r?\n\[/)[0];
assert.ok(cargoPackage, "Cargo package metadata is missing");
assert.equal(pkg.name, "astra-os");
assert.equal(lock.name, pkg.name);
assert.equal(lock.packages[""].name, pkg.name);
assert.equal(lock.packages[""].version, pkg.version);
assert.equal(config.version, pkg.version, "Tauri/package version mismatch");
assert.equal(cargoPackage.match(/^version\s*=\s*"([^"]+)"/m)?.[1], pkg.version);
assert.equal(cargoPackage.match(/^name\s*=\s*"([^"]+)"/m)?.[1], pkg.name);
assert.equal(config.productName, "Astra OS");
assert.equal(config.identifier, "com.astra.os");
assert.ok(config.bundle.icon.length > 0, "Installer icons are missing");
for (const icon of config.bundle.icon) {
  await access(path.join(root, "src-tauri", icon));
}
await access(path.join(root, "public/astra.svg"));
await access(path.join(root, "public/astra-logo.png"));
console.log(`Release metadata and icon paths verified for Astra OS ${pkg.version}.`);
