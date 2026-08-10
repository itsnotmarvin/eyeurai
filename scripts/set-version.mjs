import { readFile, writeFile } from "node:fs/promises";
import process from "node:process";

const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const files = {
  package: new URL("../package.json", import.meta.url),
  lock: new URL("../package-lock.json", import.meta.url),
  tauri: new URL("../src-tauri/tauri.conf.json", import.meta.url),
  cargo: new URL("../src-tauri/Cargo.toml", import.meta.url),
  cargoLock: new URL("../src-tauri/Cargo.lock", import.meta.url),
};

async function json(url) {
  return JSON.parse(await readFile(url, "utf8"));
}

async function versions() {
  const [packageJson, packageLock, tauri, cargo, cargoLock] = await Promise.all([
    json(files.package),
    json(files.lock),
    json(files.tauri),
    readFile(files.cargo, "utf8"),
    readFile(files.cargoLock, "utf8"),
  ]);
  const cargoVersion = cargo.match(/\[package\]\s+name = "eyeurai"\s+version = "([^"]+)"/)?.[1];
  const cargoLockVersion = cargoLock.match(/\[\[package\]\]\s+name = "eyeurai"\s+version = "([^"]+)"/)?.[1];
  return {
    package: packageJson.version,
    packageLock: packageLock.version,
    packageLockRoot: packageLock.packages?.[""]?.version,
    tauri: tauri.version,
    cargo: cargoVersion,
    cargoLock: cargoLockVersion,
  };
}

function assertVersion(version) {
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`Invalid version "${version}". Use a value such as 1.0.1 (without a leading v).`);
  }
}

async function check() {
  const found = await versions();
  const unique = new Set(Object.values(found));
  if (unique.size !== 1 || unique.has(undefined)) {
    throw new Error(`EyeUrAI versions are out of sync:\n${JSON.stringify(found, null, 2)}`);
  }
  const [version] = unique;
  assertVersion(version);
  const tag = process.env.GITHUB_REF_TYPE === "tag" ? process.env.GITHUB_REF_NAME : null;
  if (tag && tag !== `v${version}`) {
    throw new Error(`Release tag ${tag} does not match app version v${version}.`);
  }
  console.log(`EyeUrAI version ${version} is synchronized.`);
}

async function setVersion(version) {
  assertVersion(version);
  const [packageJson, packageLock, tauri, cargo, cargoLock] = await Promise.all([
    json(files.package),
    json(files.lock),
    json(files.tauri),
    readFile(files.cargo, "utf8"),
    readFile(files.cargoLock, "utf8"),
  ]);

  packageJson.version = version;
  packageLock.version = version;
  packageLock.packages[""].version = version;
  tauri.version = version;
  const nextCargo = cargo.replace(
    /(\[package\]\s+name = "eyeurai"\s+version = ")[^"]+/,
    `$1${version}`,
  );
  const nextCargoLock = cargoLock.replace(
    /(\[\[package\]\]\s+name = "eyeurai"\s+version = ")[^"]+/,
    `$1${version}`,
  );

  await Promise.all([
    writeFile(files.package, `${JSON.stringify(packageJson, null, 2)}\n`),
    writeFile(files.lock, `${JSON.stringify(packageLock, null, 2)}\n`),
    writeFile(files.tauri, `${JSON.stringify(tauri, null, 2)}\n`),
    writeFile(files.cargo, nextCargo),
    writeFile(files.cargoLock, nextCargoLock),
  ]);
  console.log(`Set EyeUrAI version to ${version}.`);
}

const argument = process.argv[2];
if (argument === "--check") await check();
else if (argument) await setVersion(argument);
else throw new Error("Pass a version, for example: npm run release:prepare -- 1.0.1");
