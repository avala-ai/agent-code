#!/usr/bin/env node

// Downloads the correct prebuilt agent-code binary for this platform.

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const https = require("https");

const REPO = "avala-ai/agent-code";
const VERSION = require("./package.json").version;
const BIN_DIR = path.join(__dirname, "bin");

const PLATFORMS = {
  "darwin-x64": { artifact: "agent-macos-x86_64", ext: ".tar.gz", binary: "agent" },
  "darwin-arm64": { artifact: "agent-macos-aarch64", ext: ".tar.gz", binary: "agent" },
  "linux-x64": { artifact: "agent-linux-x86_64", ext: ".tar.gz", binary: "agent" },
  "linux-arm64": { artifact: "agent-linux-aarch64", ext: ".tar.gz", binary: "agent" },
  "win32-x64": { artifact: "agent-windows-x86_64", ext: ".zip", binary: "agent.exe" },
};

function getPlatformKey() {
  return `${process.platform}-${process.arch}`;
}

function downloadFile(url) {
  return new Promise((resolve, reject) => {
    const follow = (url, redirects = 0) => {
      if (redirects > 5) return reject(new Error("Too many redirects"));
      const mod = url.startsWith("https") ? https : require("http");
      mod.get(url, { headers: { "User-Agent": "agent-code-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return follow(res.headers.location, redirects + 1);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`Download failed: HTTP ${res.statusCode}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      }).on("error", reject);
    };
    follow(url);
  });
}

async function main() {
  const key = getPlatformKey();
  const platform = PLATFORMS[key];

  if (!platform) {
    console.error(`Unsupported platform: ${key}`);
    console.error(`Supported: ${Object.keys(PLATFORMS).join(", ")}`);
    console.error("Install from source: cargo install agent-code");
    process.exit(1);
  }

  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${platform.artifact}${platform.ext}`;
  console.log(`Downloading agent-code v${VERSION} for ${key}...`);

  try {
    const data = await downloadFile(url);

    // Ensure bin directory exists.
    fs.mkdirSync(BIN_DIR, { recursive: true });

    if (platform.ext === ".tar.gz") {
      // Write tar.gz to temp, extract with tar.
      const tmpPath = path.join(BIN_DIR, "download.tar.gz");
      fs.writeFileSync(tmpPath, data);
      execSync(`tar xzf "${tmpPath}" -C "${BIN_DIR}"`, { stdio: "pipe" });
      fs.unlinkSync(tmpPath);
    } else if (platform.ext === ".zip") {
      // Write zip to temp, extract.
      const tmpPath = path.join(BIN_DIR, "download.zip");
      fs.writeFileSync(tmpPath, data);
      if (process.platform === "win32") {
        execSync(`powershell -Command "Expand-Archive -Path '${tmpPath}' -DestinationPath '${BIN_DIR}' -Force"`, { stdio: "pipe" });
      } else {
        execSync(`unzip -o "${tmpPath}" -d "${BIN_DIR}"`, { stdio: "pipe" });
      }
      fs.unlinkSync(tmpPath);
    }

    // Make binary executable (Unix).
    const binPath = path.join(BIN_DIR, platform.binary);
    if (process.platform !== "win32") {
      fs.chmodSync(binPath, 0o755);
    }

    console.log(`Installed agent-code v${VERSION} to ${binPath}`);
  } catch (err) {
    console.error(`Failed to install agent-code: ${err.message}`);
    console.error("Try: cargo install agent-code");
    process.exit(1);
  }
}

main();
